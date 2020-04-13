use crate::{buf_try, BufPoll, Deserialize, MinCodecRead, MinCodecWrite, Serialize};
use bitbuf::{BitBuf, BitBufMut};
use core::{convert::Infallible, marker::PhantomData, pin::Pin, task::Context};
use void::Void;

pub struct EmptyCodec<T>(PhantomData<T>);

impl<T> EmptyCodec<T> {
    pub fn new() -> Self {
        EmptyCodec(PhantomData)
    }
}

#[derive(Debug)]
pub struct Empty;

impl<T> Deserialize for EmptyCodec<T> {
    type Target = T;
    type Error = Empty;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_try!(Err(Empty))
    }
}

impl<T> Serialize for EmptyCodec<T> {
    type Error = Empty;

    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: B,
    ) -> BufPoll<Result<(), Self::Error>> {
        panic!("attempted to serialize empty type")
    }
}

impl MinCodecRead for Void {
    type Deserialize = EmptyCodec<Void>;

    fn deserialize() -> Self::Deserialize {
        EmptyCodec(PhantomData)
    }
}

impl MinCodecWrite for Void {
    type Serialize = EmptyCodec<Void>;

    fn serialize(self) -> Self::Serialize {
        EmptyCodec(PhantomData)
    }
}

impl MinCodecRead for Infallible {
    type Deserialize = EmptyCodec<Infallible>;

    fn deserialize() -> Self::Deserialize {
        EmptyCodec(PhantomData)
    }
}

impl MinCodecWrite for Infallible {
    type Serialize = EmptyCodec<Infallible>;

    fn serialize(self) -> Self::Serialize {
        EmptyCodec(PhantomData)
    }
}
