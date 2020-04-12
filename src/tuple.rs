use crate::{
    buf_ok, buf_ready, buf_try, BufPoll, Deserialize, MapDeserialize, MapSerialize, MinCodecRead,
    MinCodecWrite, Serialize,
};
use bitbuf::{BitBuf, BitBufMut};
use core::{pin::Pin, task::Context};
use void::Void;

impl<T: MinCodecRead> MinCodecRead for (T,)
where
    T::Deserialize: Unpin,
{
    type Deserialize = MapDeserialize<Void, T::Deserialize, (T,), fn(T) -> Result<(T,), Void>>;

    fn deserialize() -> Self::Deserialize {
        fn map<T>(item: T) -> Result<(T,), Void> {
            Ok((item,))
        }
        MapDeserialize::new(map)
    }
}

impl<T: MinCodecWrite> MinCodecWrite for (T,)
where
    T::Serialize: Unpin,
{
    type Serialize = MapSerialize<T, Void>;

    fn serialize(self) -> Self::Serialize {
        fn map<T>(item: (T,)) -> Result<T, Void> {
            Ok(item.0)
        }
        MapSerialize::new(self, map)
    }
}

macro_rules! tuple_impls {
    ($($deser:ident $ser:ident $sere:ident $len:expr => ($($n:tt $name:ident)+))+) => {
        $(
            #[derive(Debug)]
            pub enum $sere<$($name),+> {
                $($name($name)),+
            }

            #[allow(non_snake_case)]
            pub struct $ser<$($name: MinCodecWrite),+> {
                index: u8,
                $($name: $name::Serialize),+
            }

            #[allow(non_snake_case)]
            pub struct $deser<$($name: MinCodecRead),+> {
                index: u8,
                data: Option<($(Option<$name>,)+)>,
                $($name: $name::Deserialize),+
            }

            impl<$($name),+> Serialize for $ser<$($name,)+>
            where
                $($name: MinCodecWrite + Unpin,)+
                $($name::Serialize: Unpin,)+
            {
                type Error = $sere<
                    $(<$name::Serialize as Serialize>::Error,)+
                >;

                fn poll_serialize<B: BitBufMut>(
                    mut self: Pin<&mut Self>,
                    ctx: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<(), Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this.index {
                            $(
                                $n => {
                                    buf_try!(buf_ready!(Pin::new(&mut this.$name).poll_serialize(ctx, buf))
                                        .map_err($sere::$name));
                                    this.index += 1;
                                }
                            )+
                            a if a == $len => {
                                this.index += 1;
                                return buf_ok!(());
                            },
                            _ => panic!("tuple serialize polled after completion"),
                        }
                    }
                }
            }

            impl<$($name),+> Deserialize for $deser<$($name,)+>
            where
                $($name: MinCodecRead + Unpin,)+
                $($name::Deserialize: Unpin,)+
            {
                type Target = ($($name,)+);
                type Error = $sere<
                    $(<$name::Deserialize as Deserialize>::Error,)+
                >;

                fn poll_deserialize<B: BitBuf>(
                    mut self: Pin<&mut Self>,
                    ctx: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<($($name,)+), Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this.index {
                            $(
                                $n => {
                                    let item = buf_try!(buf_ready!(Pin::new(&mut this.$name).poll_deserialize(ctx, buf))
                                        .map_err($sere::$name));
                                    this.data.as_mut().unwrap().$n = Some(item);
                                    this.index += 1;
                                }
                            )+
                            a if a == $len => {
                                this.index += 1;
                                #[allow(non_snake_case)]
                                let ($($name,)+) = this.data.take().unwrap();
                                return buf_ok!(($($name.unwrap(),)+));
                            },
                            _ => panic!("tuple serialize polled after completion"),
                        }
                    }
                }
            }

            impl<$($name: MinCodecWrite + Unpin),+> MinCodecWrite for ($($name,)+)
            where
                $($name::Serialize: Unpin,)+
            {
                type Serialize = $ser<$($name,)+>;

                fn serialize(self) -> Self::Serialize {
                    #[allow(non_snake_case)]
                    let ($($name),+) = self;

                    $ser {
                        index: 0,
                        $($name: $name.serialize(),)+
                    }
                }
            }

            impl<$($name: MinCodecRead + Unpin),+> MinCodecRead for ($($name,)+)
            where
                $($name::Deserialize: Unpin,)+
            {
                type Deserialize = $deser<$($name,)+>;

                fn deserialize() -> Self::Deserialize {
                    $deser {
                        index: 0,
                        $($name: $name::deserialize(),)+
                        data: Some(($(None::<$name>,)+)),
                    }
                }
            }
        )+
    }
}

tuple_impls! {
    Tuple2Deserialize  Tuple2Serialize  Tuple2Error   2 => (0 T0 1 T1)
    Tuple3Deserialize  Tuple3Serialize  Tuple3Error   3 => (0 T0 1 T1 2 T2)
    Tuple4Deserialize  Tuple4Serialize  Tuple4Error   4 => (0 T0 1 T1 2 T2 3 T3)
    Tuple5Deserialize  Tuple5Serialize  Tuple5Error   5 => (0 T0 1 T1 2 T2 3 T3 4 T4)
    Tuple6Deserialize  Tuple6Serialize  Tuple6Error   6 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5)
    Tuple7Deserialize  Tuple7Serialize  Tuple7Error   7 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6)
    Tuple8Deserialize  Tuple8Serialize  Tuple8Error   8 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7)
    Tuple9Deserialize  Tuple9Serialize  Tuple9Error   9 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8)
    Tuple10Deserialize Tuple10Serialize Tuple10Error  10 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9)
    Tuple11Deserialize Tuple11Serialize Tuple11Error  11 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10)
    Tuple12Deserialize Tuple12Serialize Tuple12Error  12 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11)
    Tuple13Deserialize Tuple13Serialize Tuple13Error  13 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12)
    Tuple14Deserialize Tuple14Serialize Tuple14Error  14 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13)
    Tuple15Deserialize Tuple15Serialize Tuple15Error  15 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14)
    Tuple16Deserialize Tuple16Serialize Tuple16Error  16 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14 15 T15)
}
