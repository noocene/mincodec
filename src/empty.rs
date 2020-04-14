use crate::{buf_try, BufPoll, Deserialize, MinCodecRead, MinCodecWrite, Serialize};
use bitbuf::{BitBuf, BitBufMut};
use core::{convert::Infallible, marker::PhantomData, pin::Pin, task::Context};
use void::Void;

/// Codec for empty/bottom types
///
/// This is a codec that provides serialization and deserialization
/// for types with 0 values, i.e. divergent types. In Rust the main divergent type (return type of functions that never return) is `!`, which
/// is still yet to be promoted to a first-class type, so `core::convert::Infallible` or `void::Void` instead employ the technique
/// of representing a value that can never be constructed using an empty enum. This codec panics on an attempt at serialization (the presence of a value of a type with no values indicates undefined behavior or an invalid state)
/// and fails with an error on serialization.
pub struct EmptyCodec<T>(PhantomData<T>);

impl<T> EmptyCodec<T> {
    /// Constructs a new EmptyCodec
    pub fn new() -> Self {
        EmptyCodec(PhantomData)
    }
}

/// The error value returned when attempting to deserialize an empty type
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
