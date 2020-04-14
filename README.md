# mincodec

[Documentation](https://noocene.github.io/mincodec)

Highly spatially efficient serialization format supporting `#[no_std]` and making zero allocations.

#### comparison
All sizes listed in bits
|Type | Value | mincodec | CBOR | bincode | MessagePack|
|--- | --- | --- | --- | --- | ---|
|`Option<bool>`|`Some(true)`|2|8|16|8|
|`Option<bool>`|`Some(false)`|2|8|16|8|
|`Option<bool>`|`None`|1|8|8|8|
|`String`|`"hello"`|48|48|104|48|
|`Vec<bool>`|`[true, false, true]`|11|32|88|32|
|`(bool, u8)`|`(false, 3)`|9|24|16|24|
|`CustomOption<bool>`|`Some(true)`|2|56|40|24|
|`()`|`()`|0|8|0|8|
|`[String; 0]`|`[]`|0|8|0|8|
|`[bool; 2]`|`[true, false]`|2|24|16|24|
|`Bit`|`High`|1|40|32|24|
|`OneVariant`|`Variant`|0|64|32|24|

```Rust
pub enum CustomOption<T> {
    Some(T),
    None,
}

pub enum Bit {
    High,
    Low,
}

pub enum OneVariant {
    Variant,
}
```