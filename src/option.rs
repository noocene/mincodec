use crate::{
    buf_ok, buf_ready, buf_try, sufficient, BufPoll, Deserialize, MinCodecRead, MinCodecWrite,
    Serialize,
};
use bitbuf::{BitBuf, BitBufMut};
use core::{pin::Pin, task::Context};

enum OptionSerializeState {
    Predicate,
    Writing,
    Complete,
}

/// Write side of a codec for the `Option` type
///
/// `Some(T)` is one bit larger than `T` and `None` occupies a single bit
pub struct OptionSerialize<T: MinCodecWrite> {
    ser: Option<T::Serialize>,
    state: OptionSerializeState,
}

impl<T: MinCodecWrite> Serialize for OptionSerialize<T>
where
    T::Serialize: Unpin,
{
    type Error = <T::Serialize as Serialize>::Error;

    fn poll_serialize<B: BitBufMut>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: B,
    ) -> BufPoll<Result<(), Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                OptionSerializeState::Predicate => {
                    let is_some = this.ser.is_some();
                    sufficient!(buf.write_bool(is_some));
                    if is_some {
                        this.state = OptionSerializeState::Writing
                    } else {
                        this.state = OptionSerializeState::Complete;
                        return buf_ok!(());
                    }
                }
                OptionSerializeState::Writing => {
                    buf_try!(buf_ready!(
                        Pin::new(this.ser.as_mut().unwrap()).poll_serialize(ctx, buf)
                    ));
                    this.state = OptionSerializeState::Complete;
                    return buf_ok!(());
                }
                OptionSerializeState::Complete => panic!("OptionSerialize polled after completion"),
            }
        }
    }
}

impl<T: MinCodecWrite> MinCodecWrite for Option<T>
where
    T::Serialize: Unpin,
{
    type Serialize = OptionSerialize<T>;

    fn serialize(self) -> Self::Serialize {
        OptionSerialize {
            ser: self.map(|a| a.serialize()),
            state: OptionSerializeState::Predicate,
        }
    }
}

enum OptionDeserializeState {
    Predicate,
    Reading,
    Complete,
}

/// Read-side of a codec for the `Option` type
///
/// `Some(T)` is one bit larger than `T` and `None` occupies a single bit
pub struct OptionDeserialize<T: MinCodecRead> {
    deser: T::Deserialize,
    state: OptionDeserializeState,
}

impl<T: MinCodecRead> Deserialize for OptionDeserialize<T>
where
    T::Deserialize: Unpin,
{
    type Target = Option<T>;
    type Error = <T::Deserialize as Deserialize>::Error;

    fn poll_deserialize<B: BitBuf>(
        mut self: Pin<&mut Self>,
        ctx: &mut Context,
        mut buf: B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        let this = &mut *self;
        loop {
            match this.state {
                OptionDeserializeState::Predicate => {
                    let is_some = sufficient!(buf.read_bool());
                    if is_some {
                        this.state = OptionDeserializeState::Reading;
                    } else {
                        this.state = OptionDeserializeState::Complete;
                        return buf_ok!(None);
                    }
                }
                OptionDeserializeState::Reading => {
                    let item = buf_try!(buf_ready!(
                        Pin::new(&mut this.deser).poll_deserialize(ctx, buf)
                    ));
                    this.state = OptionDeserializeState::Complete;
                    return buf_ok!(Some(item));
                }
                OptionDeserializeState::Complete => {
                    panic!("OptionDeserialize polled after completion")
                }
            }
        }
    }
}

impl<T: MinCodecRead> MinCodecRead for Option<T>
where
    T::Deserialize: Unpin,
{
    type Deserialize = OptionDeserialize<T>;

    fn deserialize() -> Self::Deserialize {
        OptionDeserialize {
            state: OptionDeserializeState::Predicate,
            deser: T::deserialize(),
        }
    }
}
