use crate::{
    buf_ok, buf_ready, buf_try, sufficient, BufPoll, Deserialize, MinCodecRead, MinCodecWrite,
    Serialize,
};
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    string::{FromUtf8Error, String},
    vec,
    vec::{IntoIter, Vec},
};
use bitbuf::{BitBuf, BitBufMut, Drain, Fill};
use bitbuf_vlq::{AsyncReadVlq, Vlq};
use core::{convert::TryInto, mem::replace, pin::Pin, task::Context};
use pin_project::pin_project;
use void::Void;

/// Wrapper around errors that occur while deserializing a `String`
#[derive(Debug)]
pub enum StringReadError {
    /// Indicates that the deserialized string is longer than the system pointer size
    TooLong,
    /// Wraps a `FromUtf8Error` produced while validating the deserialized data as UTF-8
    Utf8(FromUtf8Error),
}

enum StringDeserializeState {
    Vlq,
    Reading,
    Complete,
}

/// Read codec for the `String` type.
///
/// This is packed with maximal efficiency but, as `String` is a dynamically sized type,
/// it is also prefixed with a variable-length quantity provided by [bitbuf-vlq](https://github.com/noocene/bitbuf-vlq) that
/// stores the length in a compact representation wasting one bit per byte (i.e. values less than `2^7` occupy one byte, values greater than `2^7 - 1` but less than `2^14` occupy two bytes, and so forth up to 9 bytes for `core::u64::MAX`).
pub struct StringDeserialize {
    state: StringDeserializeState,
    vlq: AsyncReadVlq,
    data: Fill<Vec<u8>>,
}

impl StringDeserialize {
    fn new() -> Self {
        StringDeserialize {
            state: StringDeserializeState::Vlq,
            data: Fill::new(Vec::new()),
            vlq: Vlq::async_read(),
        }
    }
}

impl Deserialize for StringDeserialize {
    type Target = String;
    type Error = StringReadError;

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        _: &mut Context,
        mut buf: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                StringDeserializeState::Vlq => {
                    let len: usize = buf_try!(sufficient!(this.vlq.poll_read(&mut buf))
                        .try_into()
                        .map_err(|_| StringReadError::TooLong));
                    this.data = Fill::new(vec![0; len]);
                    this.state = StringDeserializeState::Reading;
                }
                StringDeserializeState::Reading => {
                    sufficient!(this.data.fill_from(&mut buf));
                    this.state = StringDeserializeState::Complete;
                    let data = replace(&mut this.data, Fill::new(Vec::new())).into_inner();
                    return BufPoll::Ready(String::from_utf8(data).map_err(StringReadError::Utf8));
                }
                StringDeserializeState::Complete => {
                    panic!("String deserialize polled after completion")
                }
            }
        }
    }
}

impl MinCodecRead for String {
    type Deserialize = StringDeserialize;

    fn deserialize() -> Self::Deserialize {
        StringDeserialize::new()
    }
}

/// Generalized provider for serializing a dynamically sized bytevec
///
/// Used in serialization of `String`.
///
/// This is packed with maximal efficiency but, as Vec is a dynamically sized type,
/// it is also prefixed with a variable-length quantity provided by [bitbuf-vlq](https://github.com/noocene/bitbuf-vlq) that
/// stores the length in a compact representation wasting one bit per byte (i.e. values less than `2^7` occupy one byte, values greater than `2^7 - 1` but less than `2^14` occupy two bytes, and so forth up to 9 bytes for `core::u64::MAX`).
pub struct BytesSerialize(Drain<Vec<u8>>);

impl Serialize for BytesSerialize {
    type Error = Void;

    fn poll_serialize<B: BitBufMut>(
        mut self: Pin<&mut Self>,
        _: &mut Context,
        buf: &mut B,
    ) -> BufPoll<Result<(), Self::Error>> {
        sufficient!(self.0.drain_into(buf));
        buf_ok!(())
    }
}

impl BytesSerialize {
    fn new(data: Vec<u8>) -> Self {
        BytesSerialize(Drain::new(data))
    }
}

impl MinCodecWrite for String {
    type Serialize = BytesSerialize;

    fn serialize(self) -> Self::Serialize {
        let mut data = (&*Vlq::from(self.as_bytes().len() as u64)).to_owned();
        data.append(&mut self.into_bytes());
        BytesSerialize::new(data)
    }
}

#[derive(Debug)]
enum VecDeserializeState {
    Vlq,
    Reading,
    Complete,
}

/// Read side of a codec for the `Vec` type
///
/// This is packed with maximal efficiency but, as Vec is a dynamically sized type,
/// it is also prefixed with a variable-length quantity provided by [bitbuf-vlq](https://github.com/noocene/bitbuf-vlq) that
/// stores the length in a compact representation wasting one bit per byte (i.e. values less than `2^7` occupy one byte, values greater than `2^7 - 1` but less than `2^14` occupy two bytes, and so forth up to 9 bytes for `core::u64::MAX`).
pub struct VecDeserialize<T: MinCodecRead> {
    state: VecDeserializeState,
    vlq: AsyncReadVlq,
    len: usize,
    deser: T::Deserialize,
    data: Vec<T>,
}

enum VecSerializeState {
    Vlq,
    Writing,
    Complete,
}

/// Write side of a codec for the `Vec` type
///
/// This is packed with maximal efficiency but, as Vec is a dynamically sized type,
/// it is also prefixed with a variable-length quantity provided by [bitbuf-vlq](https://github.com/noocene/bitbuf-vlq) that
/// stores the length in a compact representation wasting one bit per byte (i.e. values less than `2^7` occupy one byte, values greater than `2^7 - 1` but less than `2^14` occupy two bytes, and so forth up to 9 bytes for `core::u64::MAX`).
pub struct VecSerialize<T: MinCodecWrite> {
    ser: Option<T::Serialize>,
    state: VecSerializeState,
    vlq: Drain<Vec<u8>>,
    data: IntoIter<T>,
}

impl<T: MinCodecWrite> Serialize for VecSerialize<T>
where
    T::Serialize: Unpin,
    T: Unpin,
{
    type Error = <T::Serialize as Serialize>::Error;

    fn poll_serialize<B: BitBufMut>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: &mut B,
    ) -> BufPoll<Result<(), <Self as Serialize>::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                VecSerializeState::Vlq => {
                    sufficient!(this.vlq.drain_into(&mut buf));
                    this.state = VecSerializeState::Writing;
                }
                VecSerializeState::Writing => {
                    if let Some(item) = this.data.next() {
                        this.ser = Some(item.serialize());
                    } else {
                        this.state = VecSerializeState::Complete;
                        return buf_ok!(());
                    }
                    buf_try!(buf_ready!(
                        Pin::new(this.ser.as_mut().unwrap()).poll_serialize(ctx, &mut buf)
                    ));
                }
                VecSerializeState::Complete => panic!("Vec serialize polled after completion"),
            }
        }
    }
}

impl<T: MinCodecRead + Unpin> MinCodecRead for Vec<T>
where
    T::Deserialize: Unpin,
{
    type Deserialize = VecDeserialize<T>;

    fn deserialize() -> Self::Deserialize {
        VecDeserialize {
            state: VecDeserializeState::Vlq,
            vlq: Vlq::async_read(),
            deser: T::deserialize(),
            len: 0,
            data: Vec::new(),
        }
    }
}

impl<T: MinCodecWrite + Unpin> MinCodecWrite for Vec<T>
where
    T::Serialize: Unpin,
{
    type Serialize = VecSerialize<T>;

    fn serialize(self) -> Self::Serialize {
        let len = self.len();
        let data = self.into_iter();
        VecSerialize {
            vlq: Drain::new((&*Vlq::from(len as u64)).to_owned()),
            ser: None,
            data,
            state: VecSerializeState::Vlq,
        }
    }
}

/// Wrapper around errors that occur while deserializing a `Vec`
#[derive(Debug)]
pub enum VecReadError<T> {
    /// Indicates that the deserialized vector is longer than the system pointer size
    TooLong,
    /// Encapsulates an error that occured while deserializing the underlying item type
    Item(T),
}

impl<T> From<T> for VecReadError<T> {
    fn from(item: T) -> Self {
        VecReadError::Item(item)
    }
}

impl<T: MinCodecRead + Unpin> Deserialize for VecDeserialize<T>
where
    T::Deserialize: Unpin,
{
    type Target = Vec<T>;
    type Error = VecReadError<<T::Deserialize as Deserialize>::Error>;

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                VecDeserializeState::Vlq => {
                    let len: usize = buf_try!(sufficient!(this.vlq.poll_read(&mut buf))
                        .try_into()
                        .map_err(|_| VecReadError::TooLong));
                    this.data.reserve_exact(len);
                    this.len = len;
                    this.state = VecDeserializeState::Reading;
                }
                VecDeserializeState::Reading => {
                    if this.data.len() == this.len {
                        this.state = VecDeserializeState::Complete;
                        return buf_ok!(replace(&mut this.data, Vec::new()));
                    }
                    this.data.push(buf_try!(buf_ready!(
                        Pin::new(&mut this.deser).poll_deserialize(ctx, &mut buf)
                    )));
                    this.deser = T::deserialize();
                }
                VecDeserializeState::Complete => panic!("Vec deserialize polled after completion"),
            }
        }
    }
}

#[doc(hidden)]
#[pin_project]
pub struct BoxSerialize<T: MinCodecWrite> {
    ser: Pin<Box<T::Serialize>>,
}

impl<T: MinCodecWrite> BoxSerialize<T> {
    fn new(input: T) -> Self {
        BoxSerialize {
            ser: Box::pin(input.serialize()),
        }
    }
}

impl<T: MinCodecWrite> Serialize for BoxSerialize<T> {
    type Error = Box<<T::Serialize as Serialize>::Error>;

    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: &mut B,
    ) -> BufPoll<Result<(), Self::Error>> {
        let this = self.project();

        buf_ok!(buf_try!(buf_ready!(this
            .ser
            .as_mut()
            .poll_serialize(ctx, buf))
        .map_err(Box::new)))
    }
}

#[doc(hidden)]
#[pin_project]
pub struct BoxDeserialize<T: MinCodecRead> {
    deser: Pin<Box<T::Deserialize>>,
}

impl<T: MinCodecRead> BoxDeserialize<T> {
    fn new() -> Self {
        BoxDeserialize {
            deser: Box::pin(T::deserialize()),
        }
    }
}

impl<T: MinCodecRead> Deserialize for BoxDeserialize<T> {
    type Target = Box<T>;
    type Error = Box<<T::Deserialize as Deserialize>::Error>;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        ctx: &mut Context,
        buf: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = self.project();

        buf_ok!(buf_try!(buf_ready!(this
            .deser
            .as_mut()
            .poll_deserialize(ctx, buf))
        .map_err(Box::new)
        .map(Box::new)))
    }
}

impl<T: MinCodecRead> MinCodecRead for Box<T> {
    type Deserialize = BoxDeserialize<T>;

    fn deserialize() -> Self::Deserialize {
        BoxDeserialize::new()
    }
}

impl<T: MinCodecWrite> MinCodecWrite for Box<T> {
    type Serialize = BoxSerialize<T>;

    fn serialize(self) -> Self::Serialize {
        BoxSerialize::new(*self)
    }
}

#[cfg(test)]
mod test {
    use crate::test::round_trip;
    use core::iter::repeat;

    #[test]
    fn empty_string() {
        round_trip("".to_owned());
    }

    #[test]
    fn non_empty_strings() {
        for len in 0..100 {
            round_trip(repeat('a').take(len * 10 + 1).collect::<String>());
        }
    }

    #[test]
    fn empty_vec() {
        round_trip(Vec::<String>::new());
        round_trip(Vec::<u8>::new());
        round_trip(Vec::<()>::new());
        round_trip(Vec::<Vec<bool>>::new());
    }

    #[test]
    fn non_empty_vecs() {
        for len in 0..10 {
            round_trip(
                repeat("hello".to_owned())
                    .take(len * 2 + 1)
                    .collect::<Vec<_>>(),
            );
            round_trip(repeat(10u8).take(len * 10 + 1).collect::<Vec<_>>());
            round_trip(repeat(()).take(len * 10 + 1).collect::<Vec<_>>());
            round_trip(repeat(false).take(len * 10 + 1).collect::<Vec<_>>());
        }
    }
}
