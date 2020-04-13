use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use synstructure::{decl_derive, AddBounds, BindStyle, Structure};

decl_derive!([MinCodec] => derive);

fn derive(mut s: Structure) -> TokenStream {
    let len = s.variants().len();
    s.bind_with(|_| BindStyle::Move);
    if len == 0 {
        s.add_bounds(AddBounds::None);
        s.gen_impl(quote! {
            extern crate mincodec;

            gen impl mincodec::MinCodecRead for @Self {
                type Deserialize = mincodec::empty::EmptyCodec<Self>;

                fn deserialize() -> Self::Deserialize {
                    mincodec::empty::EmptyCodec::new()
                }
            }

            gen impl mincodec::MinCodecWrite for @Self {
                type Serialize = mincodec::empty::EmptyCodec<Self>;

                fn serialize(self) -> Self::Serialize {
                    mincodec::empty::EmptyCodec::new()
                }
            }
        })
    } else {
        let mut arms = vec![];
        let mut ser_arms = vec![];
        let mut deser_arms = vec![];
        let mut determinant_arms = vec![];
        let mut serialize_error_variants = vec![];
        let mut deserialize_error_variants = vec![];
        let mut serialize_variants = vec![];
        let mut deserialize_variants = vec![];
        let mut bits = None;
        let mut first_field = None;
        s.add_bounds(AddBounds::Fields);
        let determinant = if len > 1 {
            let b = (len as f64).log2().ceil() as usize;
            first_field = Some(
                quote! { _DERIVE_Deserialize::_DERIVE_Determinant(mincodec::bitbuf::CappedFill::new([0u8], #b).unwrap()) },
            );
            deserialize_variants
                .push(quote! { _DERIVE_Determinant(mincodec::bitbuf::CappedFill<[u8; 1]>) });
            bits = Some(b);
            quote! {
                mincodec::bitbuf::CappedDrain<[u8; 1]>,
            }
        } else {
            TokenStream::new()
        };
        for (idx, variant) in s.variants().iter().enumerate() {
            let mut serialize_bindings = vec![];
            let mut deserialize_bindings = vec![];
            let mut fields = vec![];
            let mut ser_state_arms = vec![];
            let mut deser_state_arms = vec![];
            let drain = if bits.is_some() {
                ser_state_arms.push(quote! {
                    0 => {
                        mincodec::sufficient!(_0.drain_into(&mut buf));
                        *idx += 1;
                    }
                });
                quote! { mincodec::bitbuf::CappedDrain::new([(#idx as u8).to_be_bytes()[0].reverse_bits()], #bits).unwrap(),}
            } else {
                TokenStream::new()
            };
            let mut ty_deser = vec![];
            for (idx, binding) in variant.bindings().iter().enumerate() {
                let ident = if let Some(name) = &binding.ast().ident {
                    name.to_string()
                } else {
                    format!("{}", idx)
                };
                let ident = format_ident!("{}_{}", variant.ast().ident, ident);
                let ty = &binding.ast().ty;
                serialize_error_variants.push(quote! {
                    #ident(<<#ty as mincodec::MinCodecWrite>::Serialize as mincodec::Serialize>::Error)
                });
                deserialize_error_variants.push(quote! {
                    #ident(<<#ty as mincodec::MinCodecRead>::Deserialize as mincodec::Deserialize>::Error)
                });
                let deser_idx = idx as u8;
                let ser_idx = idx as u8 + if bits.is_some() { 1 } else { 0 };
                let b_ser = format_ident!("_{}", ser_idx);
                let b_deser = format_ident!("_{}", deser_idx);
                ser_state_arms.push(quote! {
                    #ser_idx => {
                        mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_ser).poll_serialize(ctx, &mut buf)).map_err(_DERIVE_Error::#ident));
                        *idx += 1;
                    }
                });
                deser_state_arms.push(quote! {
                    #deser_idx => {
                        mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_deser).poll_deserialize(ctx, &mut buf)).map_err(_DERIVE_Error::#ident));
                        *idx += 1;
                    }
                });
                ty_deser.push(quote! { mincodec::OptionDeserialize::<#ty>::new() });
                serialize_bindings.push(quote! {<#ty as mincodec::MinCodecWrite>::Serialize});
                deserialize_bindings.push(quote! { mincodec::OptionDeserialize<#ty> });
                let pat = &binding.binding;
                fields.push(quote! { #pat.serialize() });
            }
            let pat = variant.pat();
            let det_idx = idx as u8;
            let ident = variant.ast().ident;
            if (!first_field.is_some()) && idx == 0 {
                let mut binding_tys = vec![];
                if variant.bindings().len() != 0 {
                    binding_tys.push(quote! {
                        0u8
                    });
                }
                for binding in variant.bindings() {
                    let ty = &binding.ast().ty;
                    binding_tys.push(quote! {
                        mincodec::OptionDeserialize::<#ty>::new()
                    });
                }
                first_field = Some(quote! {
                    _DERIVE_Deserialize::#ident(#(#binding_tys,)*)
                });
            }
            if !serialize_bindings.is_empty() {
                deserialize_variants.push(quote! {
                    #ident(u8, #(#deserialize_bindings,)*)
                });
                determinant_arms.push(quote! {
                    #det_idx => {
                        ::core::mem::replace(this, _DERIVE_Deserialize::#ident(0u8, #(#ty_deser,)*));
                    }
                });
                serialize_variants.push(quote! {
                    #ident(u8, #determinant #(#serialize_bindings,)*)
                });
                let ser_binding_names = (0..(serialize_bindings.len()
                    + if bits.is_some() { 1 } else { 0 }))
                    .map(|a| format_ident!("_{}", a))
                    .collect::<Vec<_>>();
                let deser_binding_names = (0..deserialize_bindings.len())
                    .map(|a| format_ident!("_{}", a))
                    .collect::<Vec<_>>();
                ser_arms.push(quote! {
                    _DERIVE_Serialize::#ident(idx, #(#ser_binding_names,)*) => {
                        match idx {
                            #(#ser_state_arms)*
                            _ => {
                                ::core::mem::replace(this, _DERIVE_Serialize::__DERIVE_Complete);
                                return mincodec::buf_ok!(());
                            }
                        }
                    }
                });
                let c = variant.construct(|_, idx| {
                    let ident = format_ident!("_{}", idx);
                    quote! {
                        #ident.take().unwrap()
                    }
                });
                deser_arms.push(quote! {
                    _DERIVE_Deserialize::#ident(idx, #(#deser_binding_names,)*) => {
                        match idx {
                            #(#deser_state_arms)*
                            _ => {
                                return mincodec::buf_ok!(#c);
                            }
                        }
                    }
                });
                arms.push(quote! {
                    #pat => {
                        _DERIVE_Serialize::#ident(0u8, #drain #(#fields,)*)
                    }
                });
            } else {
                deserialize_variants.push(quote! {
                    #ident()
                });
                serialize_variants.push(quote! {
                    #ident(#determinant)
                });
                determinant_arms.push(quote! {
                    #det_idx => {
                        ::core::mem::replace(this, _DERIVE_Deserialize::#ident());
                    }
                });
                if bits.is_some() {
                    ser_arms.push(quote! {
                        _DERIVE_Serialize::#ident(_0) => {
                            mincodec::sufficient!(_0.drain_into(&mut buf));
                            ::core::mem::replace(this, _DERIVE_Serialize::__DERIVE_Complete);
                            return mincodec::buf_ok!(());
                        }
                    });
                } else {
                    ser_arms.push(quote! {
                        _DERIVE_Serialize::#ident() => {
                            ::core::mem::replace(this, _DERIVE_Serialize::__DERIVE_Complete);
                            return mincodec::buf_ok!(());
                        }
                    });
                }
                let c = variant.construct(|_, _| {
                    quote! {}
                });
                deser_arms.push(quote! {
                    _DERIVE_Deserialize::#ident() => {
                        return mincodec::buf_ok!(#c);
                    }
                });
                arms.push(quote! {
                    #pat => {
                        _DERIVE_Serialize::#ident(#drain)
                    }
                });
            }
        }
        let first_field = first_field.unwrap();
        serialize_variants.push(quote! {
            __DERIVE_Complete
        });
        let name = &s.ast().ident;
        let ser_message = format!(
            "derived Serialize for {} polled after completion",
            s.ast().ident
        );
        let mut stream = s.gen_impl(quote! {
            extern crate mincodec;

            pub enum _DERIVE_Serialize {
                #(#serialize_variants,)*
            }

            #[derive(Debug)]
            pub enum _DERIVE_Error {
                #(#[allow(non_camel_case_types)] #serialize_error_variants,)*
            }

            impl mincodec::Serialize for _DERIVE_Serialize {
                type Error = _DERIVE_Error;

                fn poll_serialize<B: mincodec::bitbuf::BitBufMut>(mut self: ::core::pin::Pin<&mut Self>, ctx: &mut ::core::task::Context, mut buf: B) -> mincodec::BufPoll<Result<(), Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this {
                            #(#ser_arms,)*
                            _DERIVE_Serialize::__DERIVE_Complete => panic!(#ser_message)
                        }
                    }
                }
            }

            gen impl mincodec::MinCodecWrite for @Self {
                type Serialize = _DERIVE_Serialize;

                fn serialize(self) -> Self::Serialize {
                    match self {
                        #(#arms,)*
                    }
                }
            }
        });
        let det_arm = if bits.is_some() {
            quote! {
                _DERIVE_Deserialize::_DERIVE_Determinant(determinant) => {
                    mincodec::sufficient!(determinant.fill_from(&mut buf));
                    let determinant = u8::from_be_bytes([::core::mem::replace(determinant, mincodec::bitbuf::CappedFill::new([0u8], 0).unwrap()).into_inner()[0].reverse_bits()]);
                    match determinant {
                        #(#determinant_arms)*
                        _ => panic!("invalid determinant")
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        stream.extend(s.gen_impl(quote! {
            extern crate mincodec;

            pub enum _DERIVE_Deserialize {
                #(#deserialize_variants,)*
            }

            #[derive(Debug)]
            pub enum _DERIVE_Error {
                #(#[allow(non_camel_case_types)] #deserialize_error_variants,)*
            }

            impl mincodec::Deserialize for _DERIVE_Deserialize {
                type Target = #name;
                type Error = _DERIVE_Error;

                fn poll_deserialize<B: mincodec::bitbuf::BitBuf>(mut self: ::core::pin::Pin<&mut Self>, ctx: &mut ::core::task::Context, mut buf: B) -> mincodec::BufPoll<Result<Self::Target, Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this {
                            #(#deser_arms,)*
                            #det_arm
                        }
                    }
                }
            }

            gen impl mincodec::MinCodecRead for @Self {
                type Deserialize = _DERIVE_Deserialize;

                fn deserialize() -> Self::Deserialize {
                    #first_field
                }
            }
        }));
        stream
    }
}
