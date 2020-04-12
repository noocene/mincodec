use crate::{
    buf_ok, buf_ready, buf_try, BufPoll, Deserialize, MinCodecRead, MinCodecWrite, Serialize,
};
use arrayvec::{ArrayVec, IntoIter};
use bitbuf::{BitBuf, BitBufMut};
use core::{mem::replace, pin::Pin, task::Context};

macro_rules! array_impl {
    ($($($len:literal as $deser:ident + $ser:ident)* as $ty:ty)*) => {
        $($(
            pub struct $deser<T: MinCodecRead> {
                data: ArrayVec<[T; $len]>,
                deser: T::Deserialize,
            }

            impl<T: MinCodecRead> Deserialize for $deser<T>
            where
                T::Deserialize: Unpin,
                T: Unpin,
            {
                type Target = [T; $len];
                type Error = <T::Deserialize as Deserialize>::Error;

                fn poll_deserialize<B: BitBuf>(
                    mut self: Pin<&mut Self>,
                    ctx: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<Self::Target, Self::Error>> {
                    let this = &mut *self;
                    if this.data.len() >= $len {
                        panic!("array deserialize polled after completion")
                    }
                    loop {
                        let item = buf_try!(buf_ready!(
                            Pin::new(&mut this.deser).poll_deserialize(ctx, buf)
                        ));
                        this.data.push(item);
                        if this.data.len() < this.data.capacity() {
                            this.deser = T::deserialize();
                        } else {
                            return buf_ok!(replace(&mut this.data, ArrayVec::new()).into_inner().unwrap_or_else(|_| panic!("array does not have expected size")));
                        }
                    }
                }
            }

            impl<T: MinCodecRead + Unpin> MinCodecRead for [T; $len]
            where
                T::Deserialize: Unpin,
            {
                type Deserialize = $deser<T>;

                fn deserialize() -> Self::Deserialize {
                    $deser {
                        data: ArrayVec::new(),
                        deser: T::deserialize(),
                    }
                }
            }

            pub struct $ser<T: MinCodecWrite> {
                data: IntoIter<[T; $len]>,
                ser: Option<T::Serialize>,
            }

            impl<T: MinCodecWrite> Serialize for $ser<T>
            where
                T::Serialize: Unpin,
                T: Unpin,
            {
                type Error = <T::Serialize as Serialize>::Error;

                fn poll_serialize<B: BitBufMut>(
                    mut self: Pin<&mut Self>,
                    ctx: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<(), Self::Error>> {
                    let this = &mut *self;
                    loop {
                        if let Some(item) = this.data.next() {
                            this.ser = Some(item.serialize());
                        } else {
                            return buf_ok!(());
                        }
                        buf_try!(buf_ready!(Pin::new(this.ser.as_mut().unwrap()).poll_serialize(ctx, buf)));
                    }
                }
            }

            impl<T: MinCodecWrite + Unpin> MinCodecWrite for [T; $len]
            where
                T::Serialize: Unpin,
            {
                type Serialize = $ser<T>;

                fn serialize(self) -> Self::Serialize {
                    $ser {
                        ser: None,
                        data: ArrayVec::from(self).into_iter(),
                    }
                }
            }
        )*)*
    };
}

array_impl! {
    0001 as Array0001Deserialize + Array0001Serialize
    0002 as Array0002Deserialize + Array0002Serialize
    0003 as Array0003Deserialize + Array0003Serialize
    0004 as Array0004Deserialize + Array0004Serialize
    0005 as Array0005Deserialize + Array0005Serialize
    0006 as Array0006Deserialize + Array0006Serialize
    0007 as Array0007Deserialize + Array0007Serialize
    0008 as Array0008Deserialize + Array0008Serialize
    0009 as Array0009Deserialize + Array0009Serialize
    0010 as Array0010Deserialize + Array0010Serialize
    0011 as Array0011Deserialize + Array0011Serialize
    0012 as Array0012Deserialize + Array0012Serialize
    0013 as Array0013Deserialize + Array0013Serialize
    0014 as Array0014Deserialize + Array0014Serialize
    0015 as Array0015Deserialize + Array0015Serialize
    0016 as Array0016Deserialize + Array0016Serialize
    0017 as Array0017Deserialize + Array0017Serialize
    0018 as Array0018Deserialize + Array0018Serialize
    0019 as Array0019Deserialize + Array0019Serialize
    0020 as Array0020Deserialize + Array0020Serialize
    0021 as Array0021Deserialize + Array0021Serialize
    0022 as Array0022Deserialize + Array0022Serialize
    0023 as Array0023Deserialize + Array0023Serialize
    0024 as Array0024Deserialize + Array0024Serialize
    0025 as Array0025Deserialize + Array0025Serialize
    0026 as Array0026Deserialize + Array0026Serialize
    0027 as Array0027Deserialize + Array0027Serialize
    0028 as Array0028Deserialize + Array0028Serialize
    0029 as Array0029Deserialize + Array0029Serialize
    0030 as Array0030Deserialize + Array0030Serialize
    0031 as Array0031Deserialize + Array0031Serialize
    0032 as Array0032Deserialize + Array0032Serialize
    0064 as Array0064Deserialize + Array0064Serialize
    0128 as Array0128Deserialize + Array0128Serialize
    as u8
    0256 as Array0256Deserialize + Array0256Serialize
    0512 as Array0512Deserialize + Array0512Serialize
    1024 as Array1024Deserialize + Array1024Serialize
    2048 as Array2048Deserialize + Array2048Serialize
    4096 as Array4096Deserialize + Array4096Serialize
    8192 as Array8192Deserialize + Array8192Serialize
    as u16
}
