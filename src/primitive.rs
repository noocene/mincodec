use super::*;
use core::convert::{TryFrom, TryInto};

macro_rules! impl_primitives {
    () => {
        impl_primitives! {
            numbers {
                u8 as DeserializeU8 + SerializeU8, u16 as DeserializeU16 + SerializeU16, u32 as DeserializeU32 + SerializeU32, u64 as DeserializeU64 + SerializeU64, u128 as DeserializeU128 + SerializeU128,
                i8 as DeserializeI8 + SerializeI8, i16 as DeserializeI16 + SerializeI16, i32 as DeserializeI32 + SerializeI32, i64 as DeserializeI64 + SerializeI64, i128 as DeserializeI128 + SerializeI128,
                f32 as DeserializeF32 + SerializeF32, f64 as DeserializeF64 + SerializeF64
            }
            casted {
                char as u32, usize as u64, isize as u64
            }
        }
    };
    ($($cat:ident $items:tt)*) => {
        $(impl_primitives!(cat $cat $items);)*
    };
    (cat casted { $($ty:ty as $name:ident),* }) => {
        $(
            impl MinCodecRead for $ty {
                type Deserialize = $crate::MapDeserialize<<$ty as TryFrom<$name>>::Error, <$name as MinCodecRead>::Deserialize, $ty, fn($name) -> Result<$ty, <$ty as TryFrom<$name>>::Error>>;

                fn deserialize() -> Self::Deserialize {
                    fn map(input: $name) -> Result<$ty, <$ty as TryFrom<$name>>::Error> {
                        input.try_into()
                    }
                    MapDeserialize::new(map)
                }
            }

            impl MinCodecWrite for $ty {
                type Serialize = $crate::MapSerialize<$name, Void>;

                fn serialize(self) -> Self::Serialize {
                    fn map(input: $ty) -> Result<$name, Void> {
                        Ok(input as $name)
                    }
                    MapSerialize::new(self, map)
                }
            }
        )*
    };
    (cat numbers { $($ty:ty as $deser:ident + $ser:ident),* }) => {
        $(
            pub struct $deser {
                data: Option<Fill<[u8; size_of::<$ty>()]>>,
            }

            pub struct $ser {
                data: Option<Drain<[u8; size_of::<$ty>()]>>,
            }

            impl Deserialize for $deser {
                type Target = $ty;
                type Error = Void;

                fn poll_deserialize<B: BitBuf>(
                    mut self: Pin<&mut Self>,
                    _: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<Self::Target, Self::Error>> {
                    sufficient!(self.data.as_mut().unwrap().fill_from(buf));
                    buf_ok!(<$ty>::from_be_bytes(
                        self.data.take().unwrap().into_inner(),
                    ))
                }
            }

            impl Serialize for $ser {
                type Error = Void;

                fn poll_serialize<B: BitBufMut>(
                    mut self: Pin<&mut Self>,
                    _: &mut Context,
                    buf: &mut B,
                ) -> BufPoll<Result<(), Self::Error>> {
                    sufficient!(self.data.as_mut().unwrap().drain_into(buf));
                    self.data.take().unwrap();
                    buf_ok!(())
                }
            }

            impl MinCodecRead for $ty {
                type Deserialize = $deser;

                fn deserialize() -> Self::Deserialize {
                   $deser {
                       data: Some(Fill::new([0u8; size_of::<$ty>()]))
                   }
                }
            }

            impl MinCodecWrite for $ty {
                type Serialize = $ser;

                fn serialize(self) -> Self::Serialize {
                   $ser {
                       data: Some(Drain::new(self.to_be_bytes()))
                   }
                }
            }
        )*
    };
}

impl_primitives!();

#[derive(Debug)]
pub struct BoolDeserialize;

impl Deserialize for BoolDeserialize {
    type Target = bool;
    type Error = Void;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        buf: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_ok!(sufficient!(buf.read_bool()))
    }
}

#[derive(Debug)]
pub struct BoolSerialize(bool);

impl Serialize for BoolSerialize {
    type Error = Void;

    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        _: &mut Context,
        buf: &mut B,
    ) -> BufPoll<Result<(), Self::Error>> {
        buf_ok!(sufficient!(buf.write_bool(self.0)))
    }
}

impl MinCodecRead for bool {
    type Deserialize = BoolDeserialize;

    fn deserialize() -> Self::Deserialize {
        BoolDeserialize
    }
}

impl MinCodecWrite for bool {
    type Serialize = BoolSerialize;

    fn serialize(self) -> Self::Serialize {
        BoolSerialize(self)
    }
}

mod seal {
    use super::*;

    pub trait Unit {}

    impl Unit for () {}
    impl Unit for PhantomPinned {}
    impl<T: ?Sized> Unit for PhantomData<T> {}
    impl<T> Unit for [T; 0] {}
}
use seal::Unit;

pub struct UnitCodec<T: Unit>(PhantomData<T>);

impl Deserialize for UnitCodec<()> {
    type Target = ();
    type Error = Void;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_ok!(())
    }
}

impl Deserialize for UnitCodec<PhantomPinned> {
    type Target = PhantomPinned;
    type Error = Void;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_ok!(PhantomPinned)
    }
}

impl<T> Deserialize for UnitCodec<[T; 0]> {
    type Target = [T; 0];
    type Error = Void;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_ok!([])
    }
}

impl<T: ?Sized> Deserialize for UnitCodec<PhantomData<T>> {
    type Target = PhantomData<T>;
    type Error = Void;

    fn poll_deserialize<B: BitBuf>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: &mut B,
    ) -> BufPoll<Result<Self::Target, Self::Error>> {
        buf_ok!(PhantomData)
    }
}

impl<T: Unit> Serialize for UnitCodec<T> {
    type Error = Void;

    fn poll_serialize<B: BitBufMut>(
        self: Pin<&mut Self>,
        _: &mut Context,
        _: &mut B,
    ) -> BufPoll<Result<(), Self::Error>> {
        buf_ok!(())
    }
}

impl MinCodecRead for () {
    type Deserialize = UnitCodec<()>;

    fn deserialize() -> Self::Deserialize {
        UnitCodec(PhantomData)
    }
}

impl MinCodecWrite for () {
    type Serialize = UnitCodec<()>;

    fn serialize(self) -> Self::Serialize {
        UnitCodec(PhantomData)
    }
}

impl<T> MinCodecRead for [T; 0] {
    type Deserialize = UnitCodec<[T; 0]>;

    fn deserialize() -> Self::Deserialize {
        UnitCodec(PhantomData)
    }
}

impl<T> MinCodecWrite for [T; 0] {
    type Serialize = UnitCodec<[T; 0]>;

    fn serialize(self) -> Self::Serialize {
        UnitCodec(PhantomData)
    }
}

impl<T: ?Sized> MinCodecRead for PhantomData<T> {
    type Deserialize = UnitCodec<PhantomData<T>>;

    fn deserialize() -> Self::Deserialize {
        UnitCodec(PhantomData)
    }
}

impl<T: ?Sized> MinCodecWrite for PhantomData<T> {
    type Serialize = UnitCodec<PhantomData<T>>;

    fn serialize(self) -> Self::Serialize {
        UnitCodec(PhantomData)
    }
}

impl MinCodecRead for PhantomPinned {
    type Deserialize = UnitCodec<PhantomPinned>;

    fn deserialize() -> Self::Deserialize {
        UnitCodec(PhantomData)
    }
}

impl MinCodecWrite for PhantomPinned {
    type Serialize = UnitCodec<PhantomPinned>;

    fn serialize(self) -> Self::Serialize {
        UnitCodec(PhantomData)
    }
}
