use crate::{
    buf_ok, buf_ready, buf_try, sufficient, BufPoll, Deserialize, MinCodecRead, MinCodecWrite,
    Serialize,
};
use bitbuf::{BitBuf, BitBufMut};
use core::{pin::Pin, task::Context};

enum ResultDeserializeState {
    Predicate,
    ReadingT,
    ReadingU,
    Complete,
}

/// Read side of a codec for the `Result` type
///
/// `Ok(T)` is one bit larger than `T` and `Err(E)` is one bit larger than `E`
pub struct ResultDeserialize<T: MinCodecRead, U: MinCodecRead> {
    deser_u: U::Deserialize,
    deser_t: T::Deserialize,
    state: ResultDeserializeState,
}

/// An error wrapper for `Result` codecs
///
/// The `Result` deserialization or serialization process can return an error for either type used.
/// This encapsulates both into a single type.
#[derive(Debug)]
pub enum ResultError<T, U> {
    /// Error occured on the success variant
    Ok(T),
    /// Error occured on the error variant
    Err(U),
}

impl<T: MinCodecRead, U: MinCodecRead> Deserialize for ResultDeserialize<T, U>
where
    T::Deserialize: Unpin,
    U::Deserialize: Unpin,
{
    type Target = Result<T, U>;
    type Error =
        ResultError<<T::Deserialize as Deserialize>::Error, <U::Deserialize as Deserialize>::Error>;

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                ResultDeserializeState::Predicate => {
                    let is_t = sufficient!(buf.read_bool());
                    if is_t {
                        this.state = ResultDeserializeState::ReadingT;
                    } else {
                        this.state = ResultDeserializeState::ReadingU;
                    }
                }
                ResultDeserializeState::ReadingT => {
                    let item = buf_try!(buf_ready!(
                        Pin::new(&mut this.deser_t).poll_deserialize(ctx, buf)
                    )
                    .map_err(ResultError::Ok));
                    this.state = ResultDeserializeState::Complete;
                    return buf_ok!(Ok(item));
                }
                ResultDeserializeState::ReadingU => {
                    let item = buf_try!(buf_ready!(
                        Pin::new(&mut this.deser_u).poll_deserialize(ctx, buf)
                    )
                    .map_err(ResultError::Err));
                    this.state = ResultDeserializeState::Complete;
                    return buf_ok!(Err(item));
                }
                ResultDeserializeState::Complete => {
                    panic!("OptionDeserialize polled after completion")
                }
            }
        }
    }
}

impl<T: MinCodecRead, U: MinCodecRead> MinCodecRead for Result<T, U>
where
    T::Deserialize: Unpin,
    U::Deserialize: Unpin,
{
    type Deserialize = ResultDeserialize<T, U>;

    fn deserialize() -> Self::Deserialize {
        ResultDeserialize {
            state: ResultDeserializeState::Predicate,
            deser_t: T::deserialize(),
            deser_u: U::deserialize(),
        }
    }
}

enum ResultSerializeState {
    Predicate,
    WritingT,
    WritingU,
    Complete,
}

/// Write side of a codec for the `Result` type
///
/// `Ok(T)` is one bit larger than `T` and `Err(E)` is one bit larger than `E`
pub struct ResultSerialize<T: MinCodecWrite, U: MinCodecWrite> {
    ser_t: Option<T::Serialize>,
    ser_u: Option<U::Serialize>,
    state: ResultSerializeState,
}

impl<T: MinCodecWrite, U: MinCodecWrite> Serialize for ResultSerialize<T, U>
where
    T::Serialize: Unpin,
    U::Serialize: Unpin,
{
    type Error =
        ResultError<<T::Serialize as Serialize>::Error, <U::Serialize as Serialize>::Error>;

    fn poll_serialize<B: BitBufMut>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: B,
    ) -> BufPoll<Result<(), Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                ResultSerializeState::Predicate => {
                    let is_ok = this.ser_t.is_some();
                    sufficient!(buf.write_bool(is_ok));
                    if is_ok {
                        this.state = ResultSerializeState::WritingT
                    } else {
                        this.state = ResultSerializeState::WritingU
                    }
                }
                ResultSerializeState::WritingT => {
                    buf_try!(buf_ready!(
                        Pin::new(this.ser_t.as_mut().unwrap()).poll_serialize(ctx, buf)
                    )
                    .map_err(ResultError::Ok));
                    this.state = ResultSerializeState::Complete;
                    return buf_ok!(());
                }
                ResultSerializeState::WritingU => {
                    buf_try!(buf_ready!(
                        Pin::new(this.ser_u.as_mut().unwrap()).poll_serialize(ctx, buf)
                    )
                    .map_err(ResultError::Err));
                    this.state = ResultSerializeState::Complete;
                    return buf_ok!(());
                }
                ResultSerializeState::Complete => panic!("OptionSerialize polled after completion"),
            }
        }
    }
}

impl<T: MinCodecWrite, U: MinCodecWrite> MinCodecWrite for Result<T, U>
where
    T::Serialize: Unpin,
    U::Serialize: Unpin,
{
    type Serialize = ResultSerialize<T, U>;

    fn serialize(self) -> Self::Serialize {
        let ser_t;
        let ser_u;

        match self {
            Ok(item) => {
                ser_t = Some(item.serialize());
                ser_u = None;
            }
            Err(error) => {
                ser_t = None;
                ser_u = Some(error.serialize())
            }
        }

        ResultSerialize {
            state: ResultSerializeState::Predicate,
            ser_t,
            ser_u,
        }
    }
}
