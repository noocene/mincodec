use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parenthesized, parse::Parse, parse2, parse_quote, punctuated::Punctuated, Token, Type,
    WhereClause,
};
use synstructure::{decl_derive, AddBounds, BindStyle, Structure};

struct TypeList {
    _parens: syn::token::Paren,
    list: Punctuated<Type, Token![,]>,
}

impl Parse for TypeList {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let list;
        Ok(TypeList {
            _parens: parenthesized!(list in input),
            list: Punctuated::parse_terminated(&list)?,
        })
    }
}

decl_derive!([MinCodec, attributes(bounds)] => derive);
decl_derive!([FieldDebug, attributes(bounds)] => derive_debug);

fn derive_debug(mut s: Structure) -> TokenStream {
    let mut where_clause: WhereClause = parse_quote!(where);

    add_bounds(&s, |ty| {
        where_clause.predicates.push(parse_quote! {
            #ty: ::core::fmt::Debug
        });
    });

    s.add_bounds(AddBounds::None);

    let mut format = vec![];
    for variant in s.variants() {
        format.push(variant.each(|binding| {
            let pat = format!("{}({{:?}})", variant.ast().ident);
            let binding = &binding.binding;
            quote! {
                write!(f, #pat, #binding)
            }
        }));
    }
    let fallback = if s.variants().len() == 0 {
        quote! { _ => panic!() }
    } else {
        TokenStream::new()
    };
    s.gen_impl(quote! {
        gen impl ::core::fmt::Debug for @Self #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                match self {
                    #(#format)*
                    #fallback
                }
            }
        }
    })
}

fn add_bounds(s: &Structure, mut f: impl FnMut(Type)) -> bool {
    let mut tys = vec![];

    let mut has_explicit_bounds = false;

    for attr in &s.ast().attrs {
        if attr.path == parse_quote!(bounds) {
            has_explicit_bounds = true;
            let meta: TypeList = parse2(attr.tokens.clone()).unwrap();
            for ty in meta.list {
                tys.push(ty);
            }
        }
    }

    if !has_explicit_bounds {
        tys = s
            .variants()
            .iter()
            .flat_map(|variant| variant.bindings().iter())
            .map(|binding| binding.ast().ty.clone())
            .collect();
    }

    for ty in tys {
        f(ty);
    }

    has_explicit_bounds
}

fn derive(mut s: Structure) -> TokenStream {
    let len = s.variants().len();
    s.bind_with(|_| BindStyle::Move);
    s.add_bounds(AddBounds::None);
    if len == 0 {
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
        let mut size = 0;
        let mut write_where_clause: WhereClause = parse_quote!(where);
        let mut read_where_clause: WhereClause = parse_quote!(where);
        let mut i_ty = quote! { u8 };

        let mut err_attrs = vec![];
        for attr in &s.ast().attrs {
            if attr.path == parse_quote!(bounds) {
                err_attrs.push(attr.clone());
            }
        }

        add_bounds(&s, |ty| {
            write_where_clause.predicates.push(parse_quote! {
                #ty: Unpin + mincodec::MinCodec
            });
            read_where_clause.predicates.push(parse_quote! {
                #ty: Unpin + mincodec::MinCodec
            });
            write_where_clause.predicates.push(parse_quote! {
                <#ty as mincodec::MinCodecWrite>::Serialize: Unpin
            });
            read_where_clause.predicates.push(parse_quote! {
                <#ty as mincodec::MinCodecRead>::Deserialize: Unpin
            });
        });

        let determinant = if len > 1 {
            let b = (len as f64).log2().ceil() as usize;
            if b >= 8 {
                if b >= 16 {
                    size = 2;
                    i_ty = quote! { u32 };
                } else {
                    if b >= 32 {
                        panic!("too many variants")
                    }
                    size = 1;
                    i_ty = quote! { u16 };
                }
            }
            let bytes = b / 8 + 1;
            first_field = Some(
                quote! { _DERIVE_Deserialize::_DERIVE_Determinant(mincodec::bitbuf::CappedFill::new([0u8; #bytes], #b).unwrap()) },
            );
            deserialize_variants
                .push(quote! { _DERIVE_Determinant(mincodec::bitbuf::CappedFill<[u8; #bytes]>) });
            bits = Some(b);
            quote! {
                mincodec::bitbuf::CappedDrain<[u8; #bytes]>,
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
                        mincodec::sufficient!(_0.drain_into(&mut *buf));
                        *idx += 1;
                    }
                });
                quote! {
                    {
                        let mut data = (#idx as #i_ty).to_le_bytes();
                        for byte in &mut data {
                            *byte = byte.reverse_bits();
                        }
                        mincodec::bitbuf::CappedDrain::new(data, #bits).unwrap()
                    },
                }
            } else {
                TokenStream::new()
            };
            let mut ty_deser = vec![];
            let bl = variant.bindings().len();
            let bsize;
            let b_ty = if bl < 2usize.pow(8) {
                bsize = 0;
                quote! { u8 }
            } else if bl < 2usize.pow(16) {
                bsize = 1;
                quote! { u16 }
            } else if bl < 2usize.pow(32) {
                bsize = 2;
                quote! { u32 }
            } else {
                panic!("too many bindings")
            };
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
                let deser_idx = idx;
                let ser_idx = idx + if bits.is_some() { 1 } else { 0 };
                let b_ser = format_ident!("_{}", ser_idx);
                let b_deser = format_ident!("_{}", deser_idx);
                match bsize {
                    0 => {
                        let ser_idx = ser_idx as u8;
                        let deser_idx = deser_idx as u8;
                        ser_state_arms.push(quote! {
                            #ser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_ser).poll_serialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                        deser_state_arms.push(quote! {
                            #deser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_deser).poll_deserialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                    }
                    1 => {
                        let ser_idx = ser_idx as u16;
                        let deser_idx = deser_idx as u16;
                        ser_state_arms.push(quote! {
                            #ser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_ser).poll_serialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                        deser_state_arms.push(quote! {
                            #deser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_deser).poll_deserialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                    }
                    2 => {
                        let ser_idx = ser_idx as u32;
                        let deser_idx = deser_idx as u32;
                        ser_state_arms.push(quote! {
                            #ser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_ser).poll_serialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                        deser_state_arms.push(quote! {
                            #deser_idx => {
                                mincodec::buf_try!(mincodec::buf_ready!(::core::pin::Pin::new(#b_deser).poll_deserialize(ctx, &mut *buf)).map_err(_DERIVE_Error::#ident));
                                *idx += 1;
                            }
                        });
                    }
                    _ => panic!(),
                };
                ty_deser.push(quote! { mincodec::OptionDeserialize::<#ty>::new() });
                serialize_bindings.push(quote! {<#ty as mincodec::MinCodecWrite>::Serialize});
                deserialize_bindings.push(quote! { mincodec::OptionDeserialize<#ty> });
                let pat = &binding.binding;
                fields.push(quote! { #pat.serialize() });
            }
            let pat = variant.pat();
            let det_idx = idx;
            let ident = variant.ast().ident;
            if (!first_field.is_some()) && idx == 0 {
                let mut binding_tys = vec![];
                if variant.bindings().len() != 0 {
                    binding_tys.push(quote! {
                        0 as #b_ty
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
                    #ident(#b_ty, #(#deserialize_bindings,)*)
                });
                match size {
                    0 => {
                        let det_idx = det_idx as u8;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident(0 as #b_ty, #(#ty_deser,)*);
                            }
                        });
                    }
                    1 => {
                        let det_idx = det_idx as u16;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident(0 as #b_ty, #(#ty_deser,)*);
                            }
                        });
                    }
                    2 => {
                        let det_idx = det_idx as u32;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident(0 as #b_ty, #(#ty_deser,)*);
                            }
                        });
                    }
                    _ => panic!(),
                };
                serialize_variants.push(quote! {
                    #ident(#b_ty, #determinant #(#serialize_bindings,)*)
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
                                *this = _DERIVE_Serialize::__DERIVE_Complete;
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
                        _DERIVE_Serialize::#ident(0 as #b_ty, #drain #(#fields,)*)
                    }
                });
            } else {
                deserialize_variants.push(quote! {
                    #ident()
                });
                serialize_variants.push(quote! {
                    #ident(#determinant)
                });
                match size {
                    0 => {
                        let det_idx = det_idx as u8;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident();
                            }
                        });
                    }
                    1 => {
                        let det_idx = det_idx as u16;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident();
                            }
                        });
                    }
                    2 => {
                        let det_idx = det_idx as u32;
                        determinant_arms.push(quote! {
                            #det_idx => {
                                *this = _DERIVE_Deserialize::#ident();
                            }
                        });
                    }
                    _ => panic!(),
                };
                if bits.is_some() {
                    ser_arms.push(quote! {
                        _DERIVE_Serialize::#ident(_0) => {
                            mincodec::sufficient!(_0.drain_into(&mut *buf));
                            *this = _DERIVE_Serialize::__DERIVE_Complete;
                            return mincodec::buf_ok!(());
                        }
                    });
                } else {
                    ser_arms.push(quote! {
                        _DERIVE_Serialize::#ident() => {
                            *this = _DERIVE_Serialize::__DERIVE_Complete;
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
        let (impl_gen, ty_gen, where_gen) = &mut s.ast().generics.split_for_impl();
        if let Some(clause) = where_gen {
            for predicate in &clause.predicates {
                write_where_clause.predicates.push(predicate.clone());
                read_where_clause.predicates.push(predicate.clone());
            }
        }
        let ser_message = format!(
            "derived Serialize for {} polled after completion",
            s.ast().ident
        );

        let vis = &s.ast().vis;
        let mut stream = s.gen_impl(quote! {
            extern crate mincodec;

            #vis enum _DERIVE_Serialize #impl_gen #write_where_clause {
                #(#serialize_variants,)*
            }

            #[derive(::mincodec::FieldDebug)]
            #(#err_attrs)*
            #vis enum _DERIVE_Error #impl_gen #write_where_clause {
                #(#[allow(non_camel_case_types)] #serialize_error_variants,)*
            }

            impl #impl_gen mincodec::Serialize for _DERIVE_Serialize #ty_gen #write_where_clause {
                type Error = _DERIVE_Error #ty_gen;

                fn poll_serialize<B: mincodec::bitbuf::BitBufMut>(mut self: ::core::pin::Pin<&mut Self>, ctx: &mut ::core::task::Context, mut buf: &mut B) -> mincodec::BufPoll<Result<(), Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this {
                            #(#ser_arms,)*
                            _DERIVE_Serialize::__DERIVE_Complete => panic!(#ser_message)
                        }
                    }
                }
            }

            gen impl mincodec::MinCodecWrite for @Self #write_where_clause {
                type Serialize = _DERIVE_Serialize #ty_gen;

                fn serialize(self) -> Self::Serialize {
                    match self {
                        #(#arms,)*
                    }
                }
            }
        });
        let det_arm = if bits.is_some() {
            let bytes = bits.unwrap() / 8 + 1;
            quote! {
                _DERIVE_Deserialize::_DERIVE_Determinant(determinant) => {
                    mincodec::sufficient!(determinant.fill_from(&mut *buf));
                    let mut data = ::core::mem::replace(determinant, mincodec::bitbuf::CappedFill::new([0u8; #bytes], 0).unwrap()).into_inner();
                    for byte in &mut data {
                        *byte = byte.reverse_bits();
                    }
                    let determinant = <#i_ty>::from_le_bytes(data);
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

            #vis enum _DERIVE_Deserialize #impl_gen #read_where_clause {
                #(#deserialize_variants,)*
            }

            #[derive(::mincodec::FieldDebug)]
            #(#err_attrs)*
            #vis enum _DERIVE_Error #impl_gen #read_where_clause {
                #(#[allow(non_camel_case_types)] #deserialize_error_variants,)*
            }

            impl #impl_gen mincodec::Deserialize for _DERIVE_Deserialize #ty_gen #read_where_clause {
                type Target = #name #ty_gen;
                type Error = _DERIVE_Error #ty_gen;

                fn poll_deserialize<B: mincodec::bitbuf::BitBuf>(mut self: ::core::pin::Pin<&mut Self>, ctx: &mut ::core::task::Context, mut buf: &mut B) -> mincodec::BufPoll<Result<Self::Target, Self::Error>> {
                    let this = &mut *self;
                    loop {
                        match this {
                            #(#deser_arms,)*
                            #det_arm
                        }
                    }
                }
            }

            gen impl mincodec::MinCodecRead for @Self #read_where_clause {
                type Deserialize = _DERIVE_Deserialize #ty_gen;

                fn deserialize() -> Self::Deserialize {
                    #first_field
                }
            }
        }));
        stream
    }
}
