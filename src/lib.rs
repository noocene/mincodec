#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
//! High performance, extremely compact serialization format using [bitbuf](https://github.com/noocene/bitbuf)

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
/// Utility types employed by `MinCodec` implementations for heap-allocated structures
pub mod allocated;

/// Provides codecs for fixed-size arrays
pub mod array;
/// Provides a codec for empty/bottom types
pub mod empty;
/// Provides a codec for the `Option` type
pub mod option;
/// Provides codecs for primitive/builtin types
pub mod primitive;
/// Provides a codec for the `Result` type
pub mod result;
/// Provides codecs for tuples
pub mod tuple;

/// Derives a highly compact `MinCodec` implementation for any struct or enum with fields that implement `MinCodec`
///
/// Structs are packed with no wasted space between fields and enums use the smallest possible bit packing for their determinant.
pub use derive::MinCodec;

#[doc(hidden)]
pub use derive::FieldDebug;

#[doc(hidden)]
pub use bitbuf;

use bitbuf::{BitBuf, BitBufMut, BitSlice, BitSliceMut};
use core::{
    future::Future,
    mem::replace,
    pin::Pin,
    task::{Context, Poll},
};
use core_futures_io::{AsyncRead, AsyncWrite};

#[doc(hidden)]
pub use void::Void;

/// Converts a `Result<T, Insufficient>` from `bitbuf` into a `BufPoll` and returns early on `Insufficient`
///
/// Many methods from `bitbuf` return a `Result<T, bitbuf::Insufficient>` for some `T`, indicating failure
/// of an operation due to the buffer being empty. The mincodec architecture is based on converting these failures
/// into a state machine yield that resumes once more data is available, so this convenience macro sees extensive use
/// when implementing mincodec for new types.
#[macro_export]
macro_rules! sufficient {
    ($e:expr $(,)?) => {
        match $e {
            ::core::result::Result::Ok(t) => t,
            ::core::result::Result::Err(e) => {
                let _: $crate::bitbuf::Insufficient = e;
                return $crate::BufPoll::Insufficient;
            }
        }
    };
}

/// Equivalent of `?` for `BufPoll` functions
///
/// Implementing the `Try` trait outside of libstd is unstable, so to approximate the useful behavior of the `?` operator
/// this macro is provided. It takes a `Result` and returns early on error, applying `From` in the same way as `?` and unwrapping
/// the success type.
#[macro_export]
macro_rules! buf_try {
    ($e:expr $(,)?) => {
        match $e {
            ::core::result::Result::Ok(t) => t,
            ::core::result::Result::Err(e) => {
                return $crate::BufPoll::Ready(Err(e.into()));
            }
        }
    };
}

/// Shorthand for `BufPoll::Ready(Ok())`
#[macro_export]
macro_rules! buf_ok {
    ($e:expr $(,)?) => {
        $crate::BufPoll::Ready(Ok($e))
    };
}

/// Equivalent of `futures::ready` for `BufPoll`
///
/// Returns from the current function with Pending or Insufficient for those variants or returns the item on Ready.
#[macro_export]
macro_rules! buf_ready {
    ($e:expr $(,)?) => {
        match $e {
            $crate::BufPoll::Pending => return $crate::BufPoll::Pending,
            $crate::BufPoll::Insufficient => return $crate::BufPoll::Insufficient,
            $crate::BufPoll::Ready(item) => item,
        }
    };
}

/// Analogue of `core::task::Poll` with a special case for insufficent buffers
#[derive(Debug)]
pub enum BufPoll<T> {
    /// Indicates that a dependency on some asynchronous state is pending and the task is registered for wakeup
    Pending,
    /// Indicates that more data is required to complete the operation
    Insufficient,
    /// Indicates that the operation is complete and provides the return value
    Ready(T),
}

/// Describes the manner in which a type is serialized into a bit-level buffer
pub trait Serialize {
    /// The error type produced on failure of the serialization process
    type Error;

    /// Advances the serialization process. This method should read from the buffer using partial methods or `Fill`/`CappedFill` (i.e. not `read_all`) and use the provided helper
    /// macros to return `BufPoll::Insufficient` when more data is needed or `Poll::Pending` when a future is pending and will wakeup using `ctx`.
    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<(), Self::Error>>;
}

/// Describes the manner in which a type is deserialized from a bit-level buffer
pub trait Deserialize: Sized {
    /// The type this deserializer produces
    type Target;
    /// The error type produced on failure of the deserialization process
    type Error;

    /// Advances the deserialization process. This method should write to the buffer using partial methods or `Drain`/`CappedDrain` (i.e. not `write_all`) and use the provided helper
    /// macros to return `BufPoll::Insufficient` when the buffer is full or `Poll::Pending` when a future is pending and will wakeup using `ctx`.
    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>>;
}

/// A type with an associated bit-level serialization scheme
pub trait MinCodecWrite: Sized {
    /// The serializer used for this type
    type Serialize: Serialize;

    /// Consumes the type and constructs a new serializer
    fn serialize(self) -> Self::Serialize;
}

/// A type with an associated bit-level deserialization system
pub trait MinCodecRead: Sized {
    /// The deserializer used for this type
    type Deserialize: Deserialize<Target = Self>;

    /// Constructs a new deserializer
    fn deserialize() -> Self::Deserialize;
}

/// Helper for deserializing a value and then storing it temporarily
///
/// This is used in the derive macro, for example, when composing some structure
/// with a number of independently deserialized fields of different types is necessary. Because
/// `poll_deserialize` takes `self` by mutable borrow, moving deserialized content out would
/// typically require an `Option` and `take` or unsafe code in addition to a field storing the actual deserializer.
/// This type wraps the deserializer and, when deserialization completes, switches variants to store the resultant value until
/// it is later unwrapped with `take`, thereby filling the role of those two fields that would otherwise be necessary.
pub enum OptionDeserialize<T: MinCodecRead> {
    /// Stores the deserializer while deserialization is underway
    Deserialize(T::Deserialize),
    /// Stores the resultant value once deserialization completes
    Done(T),
}

/// This is a somewhat atypical implementation of `Deserialize`. Instead of deserializing the value and returning it, it returns `()`
/// and then stores the value internally for later acqusition.
impl<T: MinCodecRead> Deserialize for OptionDeserialize<T>
where
    T: Unpin,
    T::Deserialize: Unpin,
{
    type Error = <T::Deserialize as Deserialize>::Error;
    type Target = ();

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<(), <T::Deserialize as Deserialize>::Error>> {
        let this = &mut *self;
        loop {
            match this {
                OptionDeserialize::Deserialize(deser) => {
                    let item = buf_try!(buf_ready!(Pin::new(deser).poll_deserialize(ctx, buf)));
                    replace(this, OptionDeserialize::Done(item));
                    return buf_ok!(());
                }
                OptionDeserialize::Done(_) => panic!("OptionDeserialize polled after completion"),
            }
        }
    }
}

impl<T: MinCodecRead> OptionDeserialize<T>
where
    T: Unpin,
    T::Deserialize: Unpin,
{
    /// Checks if deserialization is complete
    pub fn is_done(&self) -> bool {
        if let OptionDeserialize::Done(_) = self {
            true
        } else {
            false
        }
    }

    /// Moves a complete value out, returning `None` if deserialization is incomplete
    pub fn take(&mut self) -> Option<T> {
        if let OptionDeserialize::Done(_) = self {
            let value = replace(self, OptionDeserialize::Deserialize(T::deserialize()));
            if let OptionDeserialize::Done(item) = value {
                return Some(item);
            }
            panic!()
        } else {
            None
        }
    }

    /// Creates a new `OptionDeserialize`
    pub fn new() -> Self {
        OptionDeserialize::Deserialize(T::deserialize())
    }
}

/// Wraps errors that can occur in the `ReadImmediate` adapter
#[derive(Debug)]
pub enum ReadImmediateError<T> {
    /// Insufficient data was present in the buffer
    Insufficient,
    /// An that error occurred while deserializing the target type
    Deserialize(T),
}

/// An adapter that reads a value from a fixed buffer
///
/// This fails if insufficient data is available to deserialize an instance of the target type, or if the buffer is
/// an incomplete slice of the target type.
pub struct ReadImmediate<T: MinCodecRead, B: BitBuf>(T::Deserialize, B, bool);

impl<T: MinCodecRead, B: BitBuf> Future for ReadImmediate<T, B>
where
    T::Deserialize: Unpin,
    B: Unpin,
{
    type Output = Result<T, ReadImmediateError<<T::Deserialize as Deserialize>::Error>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if this.2 {
            panic!("ReadImmediate polled after completion")
        }
        match Pin::new(&mut this.0).poll_deserialize(ctx, &mut this.1) {
            BufPoll::Pending => Poll::Pending,
            BufPoll::Insufficient => {
                this.2 = true;
                Poll::Ready(Err(ReadImmediateError::Insufficient))
            }
            BufPoll::Ready(item) => {
                this.2 = true;
                Poll::Ready(item.map_err(ReadImmediateError::Deserialize))
            }
        }
    }
}

/// Wraps errors that can occur in the `WriteImmediate` adapter
#[derive(Debug)]
pub enum WriteImmediateError<T> {
    /// The buffer was too small to contain the value
    Insufficient,
    /// An error that occurred in the serialization of the target type
    Serialize(T),
}

/// An adapter that writes a value into a fixed buffer, failing if the buffer is too small
pub struct WriteImmediate<T: MinCodecWrite, B: BitBufMut>(T::Serialize, B, bool);

impl<T: MinCodecWrite, B: BitBufMut> Future for WriteImmediate<T, B>
where
    T::Serialize: Unpin,
    B: Unpin,
{
    type Output = Result<usize, WriteImmediateError<<T::Serialize as Serialize>::Error>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if this.2 {
            panic!("WriteImmediate polled after completion")
        }
        match Pin::new(&mut this.0).poll_serialize(ctx, &mut this.1) {
            BufPoll::Pending => Poll::Pending,
            BufPoll::Insufficient => {
                this.2 = true;
                Poll::Ready(Err(WriteImmediateError::Insufficient))
            }
            BufPoll::Ready(item) => {
                this.2 = true;
                Poll::Ready(
                    item.map_err(WriteImmediateError::Serialize)
                        .map(|_| this.1.len()),
                )
            }
        }
    }
}

/// Provides utility methods for `MinCodecRead` types
pub trait MinCodecReadExt: MinCodecRead {
    /// Deserialize an instance from the provided buffer, failing if the buffer contains insufficient data
    fn read_immediate<B: BitBuf>(buf: B) -> ReadImmediate<Self, B>;
    /// Deserialize an instance from the provided `AsyncRead`
    fn read_async_bytes<R: AsyncRead>(buf: R) -> AsyncReader<R, Self>;
}

impl<T: MinCodecRead> MinCodecReadExt for T {
    fn read_immediate<B: BitBuf>(buf: B) -> ReadImmediate<Self, B> {
        ReadImmediate(T::deserialize(), buf, false)
    }
    fn read_async_bytes<R: AsyncRead>(buf: R) -> AsyncReader<R, Self> {
        AsyncReader::new(buf)
    }
}

/// Provides utility methods for `MinCodecWrite` types
pub trait MinCodecWriteExt: MinCodecWrite {
    /// Serialize into the provided buffer, failing if the buffer is too small
    fn write_immediate<B: BitBufMut>(self, buf: B) -> WriteImmediate<Self, B>;
    /// Serialize into the provided `AsyncRead`
    fn write_async_bytes<W: AsyncWrite>(self, buf: W) -> AsyncWriter<W, Self>;
}

impl<T: MinCodecWrite> MinCodecWriteExt for T {
    fn write_immediate<B: BitBufMut>(self, buf: B) -> WriteImmediate<Self, B> {
        WriteImmediate(self.serialize(), buf, false)
    }
    fn write_async_bytes<W: AsyncWrite>(self, buf: W) -> AsyncWriter<W, Self> {
        AsyncWriter::new(buf, self)
    }
}

/// Blanket-implemented alias trait that constrains on `MinCodecRead` and `MinCodecWrite`
pub trait MinCodec: MinCodecRead + MinCodecWrite {}

impl<T: MinCodecWrite + MinCodecRead> MinCodec for T {}

enum AsyncReaderState {
    Reading,
    Deserialize,
    Complete,
}

const ASYNC_READER_BUF_SIZE: usize = 1024;

/// Helper for deserializing a `MinCodecRead` type from a `core_futures_io::AsyncRead` bytestream
pub struct AsyncReader<T: AsyncRead, U: MinCodecRead> {
    reader: T,
    buffer: [u8; ASYNC_READER_BUF_SIZE],
    cursor: usize,
    last_byte: usize,
    deserializer: U::Deserialize,
    state: AsyncReaderState,
}

enum AsyncWriterState {
    Serialize,
    Writing,
    Complete,
}

const ASYNC_WRITER_BUF_SIZE: usize = 1024;

/// Helper for serializing a `MinCodecWrite` type into a `core_futures_io::AsyncWrite` bytestream
pub struct AsyncWriter<T: AsyncWrite, U: MinCodecWrite> {
    writer: T,
    buffer: [u8; ASYNC_WRITER_BUF_SIZE],
    serializer: U::Serialize,
    cursor: usize,
    done: bool,
    state: AsyncWriterState,
}

/// Error returned when attempting to reset an incomplete AsyncReader
#[derive(Debug)]
pub struct Incomplete;

impl<T: AsyncRead, U: MinCodecRead> AsyncReader<T, U> {
    /// Creates a new `AsyncReader` from the provided `AsyncRead`
    pub fn new(reader: T) -> Self {
        AsyncReader {
            reader,
            last_byte: 0,
            buffer: [0u8; ASYNC_READER_BUF_SIZE],
            cursor: 0,
            deserializer: U::deserialize(),
            state: AsyncReaderState::Deserialize,
        }
    }

    /// Begins the deserialization process again, retaining residual data. This fails if the current item deserialization is not complete.
    pub fn reset(&mut self) -> Result<(), Incomplete> {
        if let AsyncReaderState::Complete = self.state {
            self.deserializer = U::deserialize();
            self.state = AsyncReaderState::Deserialize;
            Ok(())
        } else {
            Err(Incomplete)
        }
    }

    /// Begins the deserialization process again with a new type, retaining residual data. This fails if the current item deserialization is not complete.
    pub fn reset_as<S: MinCodecRead>(self) -> Result<AsyncReader<T, S>, Incomplete> {
        if let AsyncReaderState::Complete = self.state {
            Ok(AsyncReader {
                reader: self.reader,
                state: AsyncReaderState::Deserialize,
                deserializer: S::deserialize(),
                cursor: self.cursor,
                buffer: self.buffer,
                last_byte: self.last_byte,
            })
        } else {
            Err(Incomplete)
        }
    }
}

impl<T: AsyncWrite, U: MinCodecWrite> AsyncWriter<T, U> {
    /// Creates a new `AsyncWriter` from the provided `AsyncWrite`
    pub fn new(writer: T, data: U) -> Self {
        AsyncWriter {
            writer,
            buffer: [0u8; ASYNC_WRITER_BUF_SIZE],
            cursor: 0,
            done: false,
            serializer: data.serialize(),
            state: AsyncWriterState::Serialize,
        }
    }
}

/// Abstracts over errors that can occur in the `AsyncReader` adapter
#[derive(Debug)]
pub enum AsyncReaderError<T, U> {
    /// An error that occurred in the underlying `AsyncRead`
    Read(T),
    /// An error that occurred in deserialization
    Deserialize(U),
}

impl<T: AsyncRead + Unpin, U: MinCodecRead> Future for AsyncReader<T, U>
where
    U::Deserialize: Unpin,
{
    type Output = Result<U, AsyncReaderError<T::Error, <U::Deserialize as Deserialize>::Error>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
        loop {
            match self.state {
                AsyncReaderState::Deserialize => {
                    let this = &mut *self;
                    let mut buf = BitSlice::new(&this.buffer[..this.last_byte]);
                    buf.advance(this.cursor).unwrap();
                    let poll = Pin::new(&mut this.deserializer).poll_deserialize(ctx, &mut buf);
                    return match poll {
                        BufPoll::Pending => Poll::Pending,
                        BufPoll::Ready(item) => {
                            this.cursor += buf.len();
                            this.cursor = ((this.cursor / 8) + 1) * 8;
                            this.state = AsyncReaderState::Complete;
                            Poll::Ready(item.map_err(AsyncReaderError::Deserialize))
                        }
                        BufPoll::Insufficient => {
                            this.cursor += buf.len();
                            this.state = AsyncReaderState::Reading;
                            continue;
                        }
                    };
                }
                AsyncReaderState::Reading => {
                    let this = &mut *self;
                    let cursor_bytes = this.cursor / 8;
                    let mut read_buffer = [0u8; ASYNC_READER_BUF_SIZE];
                    return match Pin::new(&mut this.reader).poll_read(
                        ctx,
                        &mut read_buffer[..ASYNC_READER_BUF_SIZE - (this.last_byte - cursor_bytes)],
                    ) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(data) => Poll::Ready(match data {
                            Err(e) => Err(AsyncReaderError::Read(e)),
                            Ok(size) => {
                                this.buffer.copy_within(cursor_bytes..this.last_byte, 0);
                                this.cursor &= 7;
                                let mut deserialize_buf = BitSliceMut::new(&mut this.buffer);
                                let first_byte = this.last_byte - cursor_bytes;
                                deserialize_buf
                                    .advance(first_byte * 8 + this.cursor)
                                    .expect("could not advance by cursor remainder");
                                deserialize_buf
                                    .write_aligned_all(&read_buffer[..size])
                                    .unwrap();
                                this.last_byte = first_byte + size;
                                this.state = AsyncReaderState::Deserialize;
                                continue;
                            }
                        }),
                    };
                }
                AsyncReaderState::Complete => panic!("AsyncReader polled after completion"),
            }
        }
    }
}

/// Abstracts over errors that can occur in the `AsyncWriter` adapter
#[derive(Debug)]
pub enum AsyncWriterError<T, U> {
    /// An error that occured in the underlying `AsyncWrite`
    Write(T),
    /// An error that occurred in serialization
    Serialize(U),
}

impl<T: AsyncWrite + Unpin, U: MinCodecWrite> Future for AsyncWriter<T, U>
where
    U::Serialize: Unpin,
{
    type Output = Result<(), AsyncWriterError<T::WriteError, <U::Serialize as Serialize>::Error>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
        loop {
            match self.state {
                AsyncWriterState::Writing => {
                    let this = &mut *self;
                    let mut l = 0u8;
                    let re = this.cursor & 7;
                    this.cursor /= 8;
                    if this.done {
                        this.cursor += 1;
                    } else {
                        l = this.buffer[this.cursor + 1];
                    }
                    match Pin::new(&mut this.writer).poll_write(ctx, &this.buffer[..this.cursor]) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(data) => match data {
                            Ok(size) => {
                                this.cursor -= size;
                                if this.cursor == 0 {
                                    if this.done {
                                        this.state = AsyncWriterState::Complete;
                                        return Poll::Ready(Ok(()));
                                    } else {
                                        this.cursor = re;
                                        this.buffer[0] = l;
                                        this.state = AsyncWriterState::Serialize;
                                    }
                                    continue;
                                }
                            }
                            Err(e) => return Poll::Ready(Err(AsyncWriterError::Write(e))),
                        },
                    }
                }
                AsyncWriterState::Serialize => {
                    let this = &mut *self;
                    let mut buf = BitSliceMut::new(&mut this.buffer);
                    buf.advance(this.cursor).unwrap();
                    let poll = Pin::new(&mut this.serializer).poll_serialize(ctx, &mut buf);
                    return match poll {
                        BufPoll::Pending => Poll::Pending,
                        BufPoll::Ready(item) => {
                            this.cursor += buf.len();
                            this.state = AsyncWriterState::Writing;
                            this.done = true;
                            item.map_err(AsyncWriterError::Serialize)?;
                            continue;
                        }
                        BufPoll::Insufficient => {
                            this.cursor += buf.len();
                            this.state = AsyncWriterState::Writing;
                            continue;
                        }
                    };
                }
                AsyncWriterState::Complete => panic!("AsyncWriter polled after completion"),
            }
        }
    }
}

/// Mapped deserialize using a translation function
///
/// This is substantially more useful than `MapSerialize` despite serving the same corresponding role for the read-side.
/// Unlike serialization, the deserializer is required to specify the target type and, of course, perform any conversion necessary
/// to translate to that resultant type. This helper retains the closure provided and translates after deserialization is complete, abstracting
/// over the additional error that could result from the final conversion.
pub struct MapDeserialize<E, T: Deserialize, U, F: FnMut(T::Target) -> Result<U, E>> {
    map: F,
    deser: T,
}

impl<E, T: Unpin + Deserialize, U, F: Unpin + FnMut(T::Target) -> Result<U, E>>
    MapDeserialize<E, T, U, F>
{
    /// Creates a new `MapDeserialize`. The provided closure is used to convert from the deserialization type of the underlying extant deserializer to the target type.
    pub fn new<R: MinCodecRead<Deserialize = T>>(map: F) -> Self
    where
        T: Deserialize<Target = R>,
    {
        MapDeserialize {
            deser: R::deserialize(),
            map,
        }
    }
}

/// An error wrapper for MapDeserialize
#[derive(Debug)]
pub enum MapDeserializeError<T, U> {
    /// An error that occurred while attempting the conversion
    Map(T),
    /// An error that occurred while deserializing the target type
    Deserialize(U),
}

impl<E, T: Unpin + Deserialize, U, F: Unpin + FnMut(T::Target) -> Result<U, E>> Deserialize
    for MapDeserialize<E, T, U, F>
{
    type Target = U;
    type Error = MapDeserializeError<E, T::Error>;

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = &mut *self;
        buf_ok!(buf_try!((this.map)(buf_try!(buf_ready!(Pin::new(
            &mut this.deser
        )
        .poll_deserialize(ctx, buf))
        .map_err(MapDeserializeError::Deserialize)))
        .map_err(MapDeserializeError::Map)))
    }
}

enum MapSerializeState<U: MinCodecWrite, E> {
    Serialize(U::Serialize),
    Complete,
    MapError(E),
}

/// Mapped serializer using a translation function
///
/// This helper struct facilitates easier serialization of types with a conversion to another serializable type.
pub struct MapSerialize<U: MinCodecWrite, E> {
    state: MapSerializeState<U, E>,
}

impl<U: MinCodecWrite, E> MapSerialize<U, E> {
    /// Construct a new mapped serializer
    ///
    /// This takes an item to serialize and a function to perform the fallible conversion. The serializer
    /// passes through the error type so it takes this closure to facilitate easier implementation of autogenerated mapped
    /// serialization.
    pub fn new<T>(item: T, mut map: impl FnMut(T) -> Result<U, E>) -> Self {
        MapSerialize {
            state: match (map)(item) {
                Ok(ser) => MapSerializeState::Serialize(ser.serialize()),
                Err(e) => MapSerializeState::MapError(e),
            },
        }
    }
}

/// An error wrapper for MapSerialize
#[derive(Debug)]
pub enum MapSerializeError<T, U> {
    /// An error that occurred in the conversion
    Map(T),
    /// An error that occurred in serialization
    Serialize(U),
}

impl<E: Unpin, U: MinCodecWrite> Serialize for MapSerialize<U, E>
where
    U::Serialize: Unpin,
{
    type Error = MapSerializeError<E, <U::Serialize as Serialize>::Error>;

    fn poll_serialize<B: BitBufMut>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<(), Self::Error>> {
        let this = &mut *self;
        match &mut this.state {
            MapSerializeState::MapError(_) => {
                if let MapSerializeState::MapError(e) =
                    replace(&mut this.state, MapSerializeState::Complete)
                {
                    return BufPoll::Ready(Err(MapSerializeError::Map(e)));
                } else {
                    panic!("invalid state")
                }
            }
            MapSerializeState::Serialize(ser) => {
                buf_try!(buf_ready!(Pin::new(ser).poll_serialize(ctx, buf))
                    .map_err(MapSerializeError::Serialize));
                this.state = MapSerializeState::Complete;
                buf_ok!(())
            }
            MapSerializeState::Complete => panic!("polled MapSerialize after completion"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use core::fmt::Debug;
    use futures::executor::block_on;

    pub fn round_trip<T: MinCodec + Clone + PartialEq + Debug>(item: T)
    where
        T::Serialize: Unpin,
        T::Deserialize: Unpin,
        <T::Serialize as Serialize>::Error: Debug,
        <T::Deserialize as Deserialize>::Error: Debug,
    {
        block_on(async move {
            let mut buffer = [0u8; 1024];
            item.clone()
                .write_immediate(&mut BitSliceMut::new(&mut buffer))
                .await
                .unwrap();
            let deser = T::read_immediate(&mut BitSlice::new(&buffer))
                .await
                .unwrap();
            assert_eq!(deser, item);
        });
    }
}
