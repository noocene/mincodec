#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod allocated;

pub mod array;
pub mod empty;
pub mod option;
pub mod primitive;
pub mod result;
pub mod tuple;

pub use derive::MinCodec;

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

#[macro_export]
macro_rules! buf_ok {
    ($e:expr $(,)?) => {
        $crate::BufPoll::Ready(Ok($e))
    };
}

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

pub enum BufPoll<T> {
    Pending,
    Insufficient,
    Ready(T),
}

pub trait Serialize {
    type Error;

    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<(), Self::Error>>;
}

pub trait Deserialize: Sized {
    type Target;
    type Error;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>>;
}

pub trait MinCodecWrite: Sized {
    type Serialize: Serialize;

    fn serialize(self) -> Self::Serialize;
}

pub trait MinCodecRead: Sized {
    type Deserialize: Deserialize<Target = Self>;

    fn deserialize() -> Self::Deserialize;
}

pub enum OptionDeserialize<T: MinCodecRead> {
    Deserialize(T::Deserialize),
    Done(T),
}

impl<T: MinCodecRead> OptionDeserialize<T>
where
    T: Unpin,
    T::Deserialize: Unpin,
{
    pub fn poll_deserialize<B: BitBuf>(
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

    pub fn is_done(&self) -> bool {
        if let OptionDeserialize::Done(_) = self {
            true
        } else {
            false
        }
    }

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

    pub fn new() -> Self {
        OptionDeserialize::Deserialize(T::deserialize())
    }
}

#[derive(Debug)]
pub enum ReadImmediateError<T> {
    Insufficient,
    Deserialize(T),
}

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

#[derive(Debug)]
pub enum WriteImmediateError<T> {
    Insufficient,
    Serialize(T),
}

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

pub trait MinCodecReadExt: MinCodecRead {
    fn read_immediate<B: BitBuf>(buf: B) -> ReadImmediate<Self, B>;
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

pub trait MinCodecWriteExt: MinCodecWrite {
    fn write_immediate<B: BitBufMut>(self, buf: B) -> WriteImmediate<Self, B>;
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

pub trait MinCodec: MinCodecRead + MinCodecWrite {}

impl<T: MinCodecWrite + MinCodecRead> MinCodec for T {}

enum AsyncReaderState {
    Reading,
    Deserialize,
    Complete,
}

const ASYNC_READER_BUF_SIZE: usize = 1024;

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

pub struct AsyncWriter<T: AsyncWrite, U: MinCodecWrite> {
    writer: T,
    buffer: [u8; ASYNC_WRITER_BUF_SIZE],
    serializer: U::Serialize,
    cursor: usize,
    done: bool,
    state: AsyncWriterState,
}

impl<T: AsyncRead, U: MinCodecRead> AsyncReader<T, U> {
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
}

impl<T: AsyncWrite, U: MinCodecWrite> AsyncWriter<T, U> {
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

#[derive(Debug)]
pub enum AsyncReaderError<T, U> {
    Read(T),
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

#[derive(Debug)]
pub enum AsyncWriterError<T, U> {
    Write(T),
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
                        l = this.buffer[this.cursor + 1];
                    } else {
                        this.cursor += 1;
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

pub struct MapDeserialize<E, T: Deserialize, U, F: FnMut(T::Target) -> Result<U, E>> {
    map: F,
    deser: T,
}

impl<E, T: Unpin + Deserialize, U, F: Unpin + FnMut(T::Target) -> Result<U, E>>
    MapDeserialize<E, T, U, F>
{
    fn new<R: MinCodecRead<Deserialize = T>>(map: F) -> Self
    where
        T: Deserialize<Target = R>,
    {
        MapDeserialize {
            deser: R::deserialize(),
            map,
        }
    }
}

#[derive(Debug)]
pub enum MapDeserializeError<T, U> {
    Map(T),
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

pub struct MapSerialize<U: MinCodecWrite, E> {
    state: MapSerializeState<U, E>,
}

impl<U: MinCodecWrite, E> MapSerialize<U, E> {
    pub fn new<T>(item: T, mut map: impl FnMut(T) -> Result<U, E>) -> Self {
        MapSerialize {
            state: match (map)(item) {
                Ok(ser) => MapSerializeState::Serialize(ser.serialize()),
                Err(e) => MapSerializeState::MapError(e),
            },
        }
    }
}

#[derive(Debug)]
pub enum MapSerializeError<T, U> {
    Map(T),
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
