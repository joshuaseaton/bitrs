// Copyright (c) 2025 Joshua Seaton
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT#

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use syn::parse::discouraged::Speculative;
use syn::parse::{Error, Parse, ParseStream, Result};
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, Fields, GenericArgument, Ident, ItemStruct, Lit,
    MetaNameValue, Pat, Path, PathArguments, Stmt, Type, braced,
    parse_macro_input, parse_quote,
};

#[proc_macro_attribute]
pub fn bitfield_repr(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr: TokenStream2 = attr.into();
    let item: TokenStream2 = item.into();
    quote! {
        #[repr(#attr)]
        #[derive(
            Debug,
            Eq,
            PartialEq,
            ::zerocopy::Immutable,
            ::zerocopy::IntoBytes,
            ::zerocopy::TryFromBytes,
        )]
        #item
    }
    .into()
}

#[proc_macro]
pub fn layout(item: TokenStream) -> TokenStream {
    parse_macro_input!(item as Bitfields)
        .to_token_stream()
        .into()
}

//
// Parsing of the bitfld type.
//

enum BaseType {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

impl BaseType {
    const fn high_bit(&self) -> Option<usize> {
        match *self {
            Self::U8 => Some(7),
            Self::U16 => Some(15),
            Self::U32 => Some(31),
            Self::U64 => Some(63),
            Self::U128 => Some(127),
            Self::Usize => None,
        }
    }
}

struct BaseTypeDef {
    def: Type,
    ty: BaseType,
}

impl TryFrom<Type> for BaseTypeDef {
    type Error = Error;

    fn try_from(type_def: Type) -> Result<Self> {
        const INVALID_BASE_TYPE: &str =
            "base type must be an unsigned integral type";
        let Type::Path(ref path_ty) = type_def else {
            return Err(Error::new_spanned(type_def, INVALID_BASE_TYPE));
        };
        let path = &path_ty.path;
        let ty = if path.is_ident("u8") {
            BaseType::U8
        } else if path.is_ident("u16") {
            BaseType::U16
        } else if path.is_ident("u32") {
            BaseType::U32
        } else if path.is_ident("u64") {
            BaseType::U64
        } else if path.is_ident("u128") {
            BaseType::U128
        } else if path.is_ident("usize") {
            BaseType::Usize
        } else {
            return Err(Error::new_spanned(path, INVALID_BASE_TYPE));
        };
        Ok(Self { def: type_def, ty })
    }
}

struct TypeDef {
    def: ItemStruct,
    base: BaseTypeDef,
}

impl Parse for TypeDef {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut strct: ItemStruct = input.parse()?;

        // Check for any redundant derives; all other derives are forwarded. If
        // no repr is specified, then we default to repr(transparent).
        let mut repr = false;
        for attr in &strct.attrs {
            if attr.path().is_ident("derive") {
                attr.parse_nested_meta(|meta| {
                    for t in &[
                        "Copy",
                        "Clone",
                        "Debug",
                        "Default",
                        "Eq",
                        "PartialEq",
                    ] {
                        if meta.path.is_ident(t) {
                            return Err(Error::new_spanned(
                                meta.path,
                                format!("layout! already derives {t}"),
                            ));
                        }
                    }
                    Ok(())
                })?;
                continue;
            }

            if attr.path().is_ident("repr") {
                repr = true;
            }
        }
        if !repr {
            strct.attrs.push(parse_quote!(#[repr(transparent)]));
        }

        //
        let base_type = if let Fields::Unnamed(fields) = &strct.fields {
            if fields.unnamed.is_empty() {
                return Err(Error::new_spanned(
                    &fields.unnamed,
                    "no base type provided",
                ));
            }
            if fields.unnamed.len() > 1 {
                return Err(Error::new_spanned(
                    &fields.unnamed,
                    "too many tuple fields; only the base type should be provided",
                ));
            }
            BaseTypeDef::try_from(fields.unnamed.first().unwrap().ty.clone())?
        } else {
            return Err(Error::new_spanned(
                &strct.fields,
                "bitfld type must be defined as a tuple struct",
            ));
        };

        if let Some(param) = strct.generics.type_params().next() {
            return Err(Error::new_spanned(
                &param.ident,
                "only const generic parameters are supported",
            ));
        }
        if let Some(param) = strct.generics.lifetimes().next() {
            return Err(Error::new_spanned(
                param,
                "only const generic parameters are supported",
            ));
        }

        Ok(Self {
            def: strct,
            base: base_type,
        })
    }
}

//
// Parsing and binding for an individual bitfield.
//

struct Bitfield {
    span: Span,
    name: Option<Ident>,
    high_bit: usize,
    low_bit: usize,
    repr: Option<Type>,
    cfg_pointer_width: Option<String>,
    doc_attrs: Vec<Attribute>,
    unshifted: bool,

    //
    default: Option<Box<Expr>>,
}

impl Bitfield {
    const fn is_reserved(&self) -> bool {
        self.name.is_none()
    }

    fn display_name(&self) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None => "reserved".to_string(),
        }
    }

    fn display_kind(&self) -> &'static str {
        if self.bit_width() == 1 {
            "bit"
        } else {
            "field"
        }
    }

    fn display_range(&self) -> String {
        if self.bit_width() == 1 {
            format!("{}", self.low_bit)
        } else {
            format!("[{}:{}]", self.high_bit, self.low_bit)
        }
    }

    const fn bit_width(&self) -> usize {
        self.high_bit - self.low_bit + 1
    }

    fn minimum_width_integral_type(&self) -> TokenStream2 {
        match self.bit_width() {
            2..=8 => quote! {u8},
            9..=16 => quote! {u16},
            17..=32 => quote! {u32},
            33..=64 => quote! {u64},
            65..=128 => quote! {u128},
            width => panic!("unexpected integral bit width: {width}"),
        }
    }

    fn getter_and_setter(&self, ty: &TypeDef) -> TokenStream2 {
        debug_assert!(!self.is_reserved());
        if self.unshifted {
            debug_assert!(self.repr.is_none());
        }

        let cfg_attr = cfg_attr(self);
        let doc_attrs = &self.doc_attrs;
        let name = self.name.as_ref().unwrap();
        let setter_name = format_ident!("set_{}", name);
        let type_name = &ty.def.ident;

        let base_type = &ty.base.def;
        let high_bit = self.high_bit;
        let low_bit = self.low_bit;
        let shifted = !self.unshifted;

        let (get_doc, set_doc) = {
            let range = self.display_range();
            let qualifier = if shifted { "" } else { "unshifted " };
            (
                format!("The {qualifier}value of `{type_name}{range}`."),
                format!("Sets the {qualifier}value of `{type_name}{range}`."),
            )
        };

        if self.bit_width() == 1 && shifted {
            return quote! {
                #cfg_attr
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub const fn #name(&self) -> bool {
                    ::bitfld::get_bit!(self.0, #low_bit)
                }

                #cfg_attr
                #(#doc_attrs)*
                #[doc = #set_doc]
                #[inline]
                pub const fn #setter_name(&mut self, value: bool) -> &mut Self {
                    ::bitfld::set_bit!(self.0, #low_bit, value);
                    self
                }
            };
        }

        let clamped_type: TokenStream2 = if shifted {
            self.minimum_width_integral_type()
        } else {
            quote! { #base_type }
        };

        let get_clamped = quote! {
            ::bitfld::get_field!(
                #base_type,
                #clamped_type,
                #high_bit,
                #low_bit,
                #shifted,
                self.0
            )
        };

        let set_clamped = quote! {
            ::bitfld::set_field!(
                #base_type,
                #high_bit,
                #low_bit,
                #shifted,
                self.0,
                value
            )
        };

        let getter = if let Some(repr) = &self.repr {
            quote! {
                #cfg_attr
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub fn #name(&self)
                    -> ::core::result::Result<#repr, ::bitfld::InvalidBits<#clamped_type>>
                where
                    #repr: ::zerocopy::TryFromBytes,
                {
                    use ::zerocopy::IntoBytes;
                    use ::zerocopy::TryFromBytes;
                    let value = #get_clamped ;
                    #repr::try_read_from_bytes(value.as_bytes())
                        .map_err(|_| ::bitfld::InvalidBits(value))
                }
            }
        } else {
            quote! {
                #cfg_attr
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub const fn #name(&self) -> #clamped_type {
                    #get_clamped
                }
            }
        };

        let setter = if let Some(repr) = &self.repr {
            quote! {
                #cfg_attr
                #[doc = #set_doc]
                #[inline]
                pub fn #setter_name(&mut self, value: #repr) -> &mut Self
                where
                    #repr: ::zerocopy::IntoBytes + ::zerocopy::Immutable
                 {
                    use ::zerocopy::IntoBytes;
                    use ::zerocopy::FromBytes;
                    const { assert!(::core::mem::size_of::<#repr>() == ::core::mem::size_of::<#clamped_type>()) }
                    let value = #clamped_type::read_from_bytes(value.as_bytes()).unwrap() as #base_type;
                    #set_clamped ;
                    self
                }
            }
        } else {
            quote! {
                #cfg_attr
                #[doc = #set_doc]
                #[inline]
                pub const fn #setter_name(&mut self, value: #clamped_type) -> &mut Self {
                    let value = value as #base_type;
                    #set_clamped ;
                    self
                }
            }
        };

        quote! {
            #cfg_attr
            #getter
            #cfg_attr
            #setter
        }
    }
}

fn cfg_attr(field: &Bitfield) -> Option<TokenStream2> {
    field.cfg_pointer_width.as_ref().map(|w| {
        quote! { #[cfg(target_pointer_width = #w)] }
    })
}

impl Parse for Bitfield {
    fn parse(input: ParseStream) -> Result<Self> {
        const INVALID_BITFIELD_DECL_FORM: &str = "bitfield declaration should take one of the following forms:\n\
            * `let $name: Bit<$bit> (= $default)?;`\n\
            * `let $name: Bits<$high, $low (, $repr)?> (= $default)?;`\n\
            * `let _: Bit<$bit> (= $value)?;`\n\
            * `let _: Bits<$high, $low> (= $value)?;`";
        let err = |spanned: &dyn ToTokens| {
            Error::new_spanned(spanned, INVALID_BITFIELD_DECL_FORM)
        };

        let stmt = input.parse::<Stmt>()?;
        let Stmt::Local(ref local) = stmt else {
            return Err(err(&stmt));
        };

        let mut doc_attrs = Vec::new();
        let mut unshifted = false;
        for attr in &local.attrs {
            if attr.path().is_ident("doc") {
                doc_attrs.push(attr.clone());
            } else if attr.path().is_ident("unshifted") {
                if unshifted {
                    return Err(Error::new_spanned(
                        attr,
                        "duplicate `#[unshifted]` attribute",
                    ));
                }
                unshifted = true;
            } else {
                return Err(Error::new_spanned(
                    attr,
                    "attributes are not permitted on individual fields",
                ));
            }
        }

        let Pat::Type(ref pat_type) = local.pat else {
            return Err(err(&local));
        };

        let name: Option<Ident> = match *pat_type.pat {
            Pat::Ident(ref pat_ident) => {
                if let Some(by_ref) = &pat_ident.by_ref {
                    return Err(err(by_ref));
                }
                if let Some(mutability) = &pat_ident.mutability {
                    return Err(err(mutability));
                }
                if let Some(subpat) = &pat_ident.subpat {
                    return Err(err(&subpat.0));
                }
                Some(pat_ident.ident.clone())
            }
            Pat::Wild(_) => None,
            _ => return Err(err(&*pat_type.pat)),
        };

        let path: &Path = if let Type::Path(ref type_path) = *pat_type.ty {
            if type_path.qself.is_some() {
                return Err(err(&*pat_type.ty));
            }
            &type_path.path
        } else {
            return Err(err(&*pat_type.ty));
        };

        let get_bits_and_repr = |bits: &mut [usize]| -> Result<Option<Type>> {
            let args = &path.segments.first().unwrap().arguments;
            let args = if let PathArguments::AngleBracketed(bracketed) = args {
                &bracketed.args
            } else {
                return Err(err(&args));
            };
            if args.len() < bits.len() || args.len() > bits.len() + 1 {
                return Err(err(&args));
            }
            for (i, bit) in bits.iter_mut().enumerate() {
                let arg = args.get(i).unwrap();
                match arg {
                    GenericArgument::Const(Expr::Lit(ExprLit {
                        lit: Lit::Int(b),
                        ..
                    })) => {
                        *bit = b.base10_parse()?;
                    }
                    _ => return Err(err(&arg)),
                }
            }
            if args.len() == bits.len() + 1 {
                let arg = args.last().unwrap();
                if let GenericArgument::Type(repr) = arg {
                    Ok(Some(repr.clone()))
                } else {
                    Err(err(&arg))
                }
            } else {
                Ok(None)
            }
        };

        let type_ident = &path.segments.first().unwrap().ident;
        let (high, low, repr) = if type_ident == "Bits" {
            let mut bits = [0usize; 2];
            let repr = get_bits_and_repr(&mut bits)?;
            if bits[0] < bits[1] {
                Err(Error::new_spanned(
                    &path.segments,
                    "first high bit, then low",
                ))
            } else {
                Ok((bits[0], bits[1], repr))
            }
        } else if type_ident == "Bit" {
            let mut bit = [0usize; 1];
            let repr = get_bits_and_repr(&mut bit)?;
            Ok((bit[0], bit[0], repr))
        } else {
            Err(err(path))
        }?;

        let default_or_value = if let Some(ref init) = local.init {
            if init.diverge.is_some() {
                return Err(err(local));
            }
            Some(init.expr.clone())
        } else {
            None
        };

        if !doc_attrs.is_empty() && name.is_none() {
            return Err(Error::new_spanned(
                &doc_attrs[0],
                "doc comments are not permitted on reserved fields",
            ));
        }

        if repr.is_some() {
            if name.is_none() {
                return Err(Error::new_spanned(
                    repr,
                    "custom representations are not permitted for reserved fields",
                ));
            }
            if high == low {
                return Err(Error::new_spanned(
                    repr,
                    "custom representations are not permitted for bits",
                ));
            }
        }

        if unshifted && name.is_none() {
            return Err(Error::new_spanned(
                &local.pat,
                "`#[unshifted]` is not permitted on reserved fields",
            ));
        }
        if unshifted && repr.is_some() {
            return Err(Error::new_spanned(
                repr,
                "`#[unshifted]` is not permitted on fields with custom representations",
            ));
        }

        Ok(Bitfield {
            span: stmt.span(),
            name,
            high_bit: high,
            low_bit: low,
            repr,
            cfg_pointer_width: None,
            doc_attrs,
            unshifted,
            default: default_or_value,
        })
    }
}

/// Parses a `#[cfg(target_pointer_width = "...")]` attribute, returning the
/// width value on success.
fn parse_target_pointer_width_cfg(attr: &Attribute) -> Result<String> {
    const ERROR_MSG: &str = "expected #[cfg(target_pointer_width = \"...\")]";
    let meta = attr
        .meta
        .require_list()
        .map_err(|_| Error::new_spanned(attr, ERROR_MSG))?;
    let cfg: MetaNameValue = meta
        .parse_args()
        .map_err(|_| Error::new_spanned(attr, ERROR_MSG))?;
    if !cfg.path.is_ident("target_pointer_width") {
        return Err(Error::new_spanned(attr, ERROR_MSG));
    }
    match cfg.value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        _ => Err(Error::new_spanned(attr, ERROR_MSG)),
    }
}

struct Bitfields {
    ty: TypeDef,
    named: Vec<Bitfield>,
    reserved: Vec<Bitfield>,
    errors: Vec<Error>,
}

impl Bitfields {
    fn constants(&self) -> TokenStream2 {
        let base = &self.ty.base.def;
        let is_usize = matches!(self.ty.base.ty, BaseType::Usize);

        let mut field_constants = Vec::new();
        let mut field_metadata = Vec::new();
        let mut num_field_stmts = Vec::new();
        let mut checks = Vec::new();
        let mut default_stmts = Vec::new();

        for field in &self.named {
            let cfg_attr = cfg_attr(field);
            let name_lower = field.name.as_ref().unwrap().to_string();
            let name_upper = name_lower.to_uppercase();
            let high_bit = field.high_bit;
            let low_bit = Literal::usize_unsuffixed(field.low_bit);
            let shifted_mask =
                quote! {::bitfld::shifted_mask!(#base, #high_bit, #low_bit) };

            let mask_name = format_ident!("{name_upper}_MASK");
            let mask_doc = format!("Unshifted bitmask of `{name_lower}`.");
            let shift_name = format_ident!("{name_upper}_SHIFT");
            let shift_doc =
                format!("Bit shift (i.e., the low bit) of `{name_lower}`.");

            field_constants.push(quote! {
                #cfg_attr
                #[doc = #mask_doc]
                pub const #mask_name: #base = (#shifted_mask << #low_bit);
                #cfg_attr
                #[doc = #shift_doc]
                pub const #shift_name: usize = #low_bit;
            });

            if let Some(default) = &field.default {
                let default_name = format_ident!("{name_upper}_DEFAULT");
                let doc = format!(
                    "Pre-shifted default value of the `{name_lower}` field.",
                );
                field_constants.push(quote! {
                    #cfg_attr
                    #[doc = #doc]
                    pub const #default_name: #base = ((#default) as #base) << #low_bit;
                });
                checks.push(quote! {
                    #cfg_attr
                    const { assert!(((#default) as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
                });
                default_stmts.push(quote! {
                    #cfg_attr
                    { v |= Self::#default_name; }
                });
            }

            if is_usize {
                let high_bit = Literal::usize_unsuffixed(field.high_bit);
                checks.push(quote! {
                    #cfg_attr
                    const { assert!(#high_bit < usize::BITS as usize) }
                });
            }

            let high_bit = Literal::usize_unsuffixed(field.high_bit);
            let default = if let Some(default) = &field.default {
                quote! { #default }
            } else {
                quote! { 0 }
            };
            field_metadata.push(quote! {
                #cfg_attr
                ::bitfld::FieldMetadata::<#base>{
                    name: #name_lower,
                    high_bit: #high_bit,
                    low_bit: #low_bit,
                    default: #default as #base,
                },
            });
            num_field_stmts.push(quote! {
                #cfg_attr
                { n += 1; }
            });
        }

        let mut rsvd1_stmts = Vec::new();
        let mut rsvd0_stmts = Vec::new();
        for rsvd in &self.reserved {
            let cfg_attr = cfg_attr(rsvd);
            let rsvd_value = rsvd.default.as_ref().unwrap();
            let high_bit = rsvd.high_bit;
            let low_bit = Literal::usize_unsuffixed(rsvd.low_bit);
            let shifted_mask =
                quote! {::bitfld::shifted_mask!(#base, #high_bit, #low_bit) };
            let name = format_ident!("RSVD_{}_{}", rsvd.high_bit, rsvd.low_bit);

            field_constants.push(quote! {
                #cfg_attr
                const #name: #base = (#rsvd_value as #base) << #low_bit;
            });
            checks.push(quote! {
                #cfg_attr
                const { assert!((#rsvd_value as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
            });
            rsvd1_stmts.push(quote! {
                #cfg_attr
                { v |= Self::#name; }
            });
            rsvd0_stmts.push(quote! {
                #cfg_attr
                { v |= !Self::#name & (#shifted_mask << #low_bit); }
            });

            if is_usize {
                let high_bit = Literal::usize_unsuffixed(rsvd.high_bit);
                checks.push(quote! {
                    #cfg_attr
                    const { assert!(#high_bit < usize::BITS as usize) }
                });
            }
        }

        let num_fields_expr = quote! {
            {
                let mut n = 0usize;
                #(#num_field_stmts)*
                n
            }
        };
        field_constants.push(quote! {
            #[doc(hidden)]
            const NUM_FIELDS: usize = #num_fields_expr;
            /// Metadata of all named fields in the layout.
            pub const FIELDS: [::bitfld::FieldMetadata::<#base>; #num_fields_expr] = [
                #(#field_metadata)*
            ];
        });

        let check_fn = if checks.is_empty() {
            quote! {}
        } else {
            let checks = checks.into_iter();
            quote! {
                #[forbid(overflowing_literals)]
                const fn check_defaults() -> () {
                    #(#checks)*
                }
            }
        };

        quote! {
            /// Mask of all reserved-as-1 bits.
            pub const RSVD1_MASK: #base = {
                let mut v: #base = 0;
                #(#rsvd1_stmts)*
                v
            };
            /// Mask of all reserved-as-0 bits.
            pub const RSVD0_MASK: #base = {
                let mut v: #base = 0;
                #(#rsvd0_stmts)*
                v
            };
            /// The default value of the layout, combining all field
            /// defaults and reserved-as values.
            pub const DEFAULT: #base = {
                let mut v: #base = Self::RSVD1_MASK;
                #(#default_stmts)*
                v
            };

            #(#field_constants)*

            #check_fn
        }
    }

    fn iter_impl(&self) -> TokenStream2 {
        let ty = &self.ty.def.ident;
        let base = &self.ty.base.def;
        let iter_type = format_ident!("{}Iter", ty);
        let vis = &self.ty.def.vis;

        let generics = &self.ty.def.generics;
        let (impl_generics, ty_generics, where_clause) =
            generics.split_for_impl();

        let mut ref_generics = generics.clone();
        ref_generics.params.insert(0, parse_quote!('a));
        let (ref_impl_generics, _, _) = ref_generics.split_for_impl();

        quote! {
            #[doc(hidden)]
            #vis struct #iter_type #impl_generics (#base, usize, usize) #where_clause;

            impl #impl_generics ::core::iter::Iterator for #iter_type #ty_generics #where_clause {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);

                fn next(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    let metadata = &<#ty #ty_generics>::FIELDS[self.1];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    self.1 += 1;
                    Some((metadata, value))
                }
            }

            impl #impl_generics ::core::iter::DoubleEndedIterator for #iter_type #ty_generics #where_clause {
                fn next_back(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    self.2 -= 1;
                    let metadata = &<#ty #ty_generics>::FIELDS[self.2];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    Some((metadata, value))
                }
            }

            impl #impl_generics #ty #ty_generics #where_clause {
                /// Returns an iterator over
                /// ([metadata][`bitfld::FieldMetadata`], value) pairs for each
                /// field.
                pub fn iter(&self) -> #iter_type #ty_generics {
                    #iter_type(self.0, 0, Self::NUM_FIELDS)
                }
            }

            impl #impl_generics ::core::iter::IntoIterator for #ty #ty_generics #where_clause {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type #ty_generics;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, Self::NUM_FIELDS) }
            }

            impl #ref_impl_generics ::core::iter::IntoIterator for &'a #ty #ty_generics #where_clause {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type #ty_generics;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, <#ty #ty_generics>::NUM_FIELDS) }
            }
        }
    }

    fn getters_and_setters(&self) -> impl Iterator<Item = TokenStream2> + '_ {
        self.named
            .iter()
            .map(|field| field.getter_and_setter(&self.ty))
    }

    fn fmt_fn(&self, integral_specifier: &str) -> TokenStream2 {
        let ty_str = &self.ty.def.ident.to_string();

        let mut custom_repr_fields = self
            .named
            .iter()
            .filter(|field| field.repr.is_some())
            .peekable();

        let where_clause = if custom_repr_fields.peek().is_some() {
            let bounds = custom_repr_fields.map(|field| {
                let repr = field.repr.as_ref().unwrap();
                quote! {#repr: ::core::fmt::Debug,}
            });
            quote! {
                where
                    #(#bounds)*
            }
        } else {
            quote! {}
        };

        let fmt_fields = self.named.iter().map(|field| {
            let cfg_attr = cfg_attr(field);
            let name = &field.name;
            let name_str = name.as_ref().unwrap().to_string();
            let default_specifier = if field.bit_width() == 1 {
                ""
            } else {
                integral_specifier
            };
            if field.repr.is_some() {
                let ok_format_string =
                    format!("{{indent}}{name_str}: {{:#?}},{{sep}}");
                let ok_format_string = Literal::string(&ok_format_string);
                let err_format_string = format!(
                    "{{indent}}{name_str}: InvalidBits({{{default_specifier}}}),{{sep}}"
                );
                let err_format_string = Literal::string(&err_format_string);
                quote! {
                    #cfg_attr
                    {
                        match self.#name() {
                            Ok(value) => write!(f, #ok_format_string, value),
                            Err(invalid) => write!(f, #err_format_string, invalid.0),
                        }?;
                    }
                }
            } else {
                let format_string = format!(
                    "{{indent}}{name_str}: {{{default_specifier}}},{{sep}}"
                );
                let format_string = Literal::string(&format_string);
                quote! {
                    #cfg_attr
                    { write!(f, #format_string, self.#name())?; }
                }
            }
        });

        quote! {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result
            #where_clause
            {
                let (sep, indent) = if f.alternate() {
                    ('\n', "    ")
                } else {
                    (' ', "")
                };
                write!(f, "{} {{{sep}", #ty_str)?;
                #(#fmt_fields)*
                write!(f, "}}")
            }
        }
    }

    fn fmt_impls(&self) -> TokenStream2 {
        let ty = &self.ty.def.ident;
        let (impl_generics, ty_generics, where_clause) =
            self.ty.def.generics.split_for_impl();
        let lower_hex_fmt = self.fmt_fn(":#x");
        let upper_hex_fmt = self.fmt_fn(":#X");
        let binary_fmt = self.fmt_fn(":#b");
        let octal_fmt = self.fmt_fn(":#o");
        quote! {
            impl #impl_generics ::core::fmt::Debug for #ty #ty_generics #where_clause {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    ::core::fmt::LowerHex::fmt(self, f)
                }
            }

            impl #impl_generics ::core::fmt::Binary for #ty #ty_generics #where_clause {
                #binary_fmt
            }

            impl #impl_generics ::core::fmt::LowerHex for #ty #ty_generics #where_clause {
                #lower_hex_fmt
            }

            impl #impl_generics ::core::fmt::UpperHex for #ty #ty_generics #where_clause {
                #upper_hex_fmt
            }

            impl #impl_generics ::core::fmt::Octal for #ty #ty_generics #where_clause {
                #octal_fmt
            }
        }
    }
}

impl Parse for Bitfields {
    fn parse(input: ParseStream) -> Result<Self> {
        let input = {
            let content;
            braced!(content in input);
            content
        };

        let ty = input.parse::<TypeDef>()?;

        let input = {
            let content;
            braced!(content in input);
            content
        };

        let mut fields = Vec::new();
        let mut errors = Vec::new();
        let is_usize = matches!(ty.base.ty, BaseType::Usize);

        // Phase 1: parse cfg blocks. Each is `#[cfg(...)] { fields }`.
        // We use a fork to distinguish a cfg block from a bare field with
        // a stray attribute (which the field parser will reject).
        let mut seen_widths = HashSet::new();
        while input.peek(syn::Token![#]) {
            let fork = input.fork();
            let attrs = fork.call(Attribute::parse_outer)?;
            if !fork.peek(syn::token::Brace) {
                break;
            }
            input.advance_to(&fork);

            let attr = &attrs[0];
            if attrs.len() > 1 {
                return Err(Error::new_spanned(
                    &attrs[1],
                    "expected `{` after cfg attribute",
                ));
            }
            let width = parse_target_pointer_width_cfg(attr)?;

            if !is_usize {
                errors.push(Error::new_spanned(
                    attr,
                    "#[cfg] blocks are only permitted in `usize`-based layouts",
                ));
            }
            if seen_widths.contains(width.as_str()) {
                errors.push(Error::new_spanned(
                    attr,
                    format!(
                        "duplicate cfg block for target_pointer_width = \"{width}\""
                    ),
                ));
            }

            let block;
            braced!(block in input);
            if block.is_empty() {
                errors.push(Error::new_spanned(
                    attr,
                    "cfg block must contain at least one field",
                ));
            }
            while !block.is_empty() {
                let mut field = block.parse::<Bitfield>()?;
                field.cfg_pointer_width = Some(width.clone());
                fields.push(field);
            }
            seen_widths.insert(width);
        }

        // Phase 2: bare fields.
        while !input.is_empty() {
            fields.push(input.parse::<Bitfield>()?);
        }

        // Propagate doc comments and `unshifted` across cfg-gated field
        // variants: if one variant has them and the other doesn't, copy over.
        for i in 0..fields.len() {
            if fields[i].name.is_none()
                || fields[i].cfg_pointer_width.is_none()
                || (fields[i].doc_attrs.is_empty() && !fields[i].unshifted)
            {
                continue;
            }
            for j in (i + 1)..fields.len() {
                if fields[j].name == fields[i].name
                    && fields[j].cfg_pointer_width.is_some()
                    && fields[j].cfg_pointer_width
                        != fields[i].cfg_pointer_width
                {
                    if !fields[i].doc_attrs.is_empty()
                        && fields[j].doc_attrs.is_empty()
                    {
                        #[allow(clippy::assigning_clones)]
                        {
                            fields[j].doc_attrs = fields[i].doc_attrs.clone();
                        }
                    }
                    if fields[i].unshifted {
                        fields[j].unshifted = true;
                    }
                }
            }
        }

        fields.sort_by_key(|field| field.low_bit);

        for i in 0..fields.len() {
            let curr = &fields[i];

            // The might be multiple overlapping fields, and some might be valid
            // if mutually excluded due to differing target pointer cfg
            // conditions.
            for next in fields.iter().skip(i + 1) {
                if curr.high_bit < next.low_bit {
                    break;
                }
                let can_overlap = curr.cfg_pointer_width.is_some()
                    && next.cfg_pointer_width.is_some()
                    && curr.cfg_pointer_width != next.cfg_pointer_width;
                if !can_overlap {
                    // TODO(https://github.com/rust-lang/rust/issues/54725): It
                    // would be nice to Span::join() the two spans, but that's still
                    // experimental.
                    errors.push(Error::new(
                        next.span,
                        format!(
                            "{} ({} {}) overlaps with {} ({} {})",
                            next.display_name(),
                            next.display_kind(),
                            next.display_range(),
                            curr.display_name(),
                            curr.display_kind(),
                            curr.display_range(),
                        ),
                    ));
                }
            }
        }

        if let Some(highest_possible) = ty.base.ty.high_bit()
            && let Some(highest) = fields.last()
            && highest.high_bit > highest_possible
        {
            errors.push(Error::new(
                highest.span,
                format!(
                    "high bit {} exceeds the highest possible value \
                     of {highest_possible}",
                    highest.high_bit
                ),
            ));
        }

        let mut bitfld = Self {
            ty,
            named: vec![],
            reserved: vec![],
            errors,
        };

        while let Some(field) = fields.pop() {
            if field.is_reserved() {
                if field.default.is_some() {
                    bitfld.reserved.push(field);
                }
            } else {
                bitfld.named.push(field);
            }
        }

        Ok(bitfld)
    }
}

impl ToTokens for Bitfields {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let type_def = &self.ty.def;
        let type_name = &type_def.ident;
        let base = &self.ty.base.def;

        let (impl_generics, ty_generics, where_clause) =
            type_def.generics.split_for_impl();

        if !self.errors.is_empty() {
            let errors = self.errors.iter().map(Error::to_compile_error);
            quote! {
                #[derive(Copy, Clone, Eq, PartialEq)]
                #type_def
                #(#errors)*
            }
            .to_tokens(tokens);
            return;
        }

        let constants = self.constants();
        let getters_and_setters = self.getters_and_setters();
        let iter_impl = self.iter_impl();
        let fmt_impls = self.fmt_impls();
        quote! {
            #[derive(Copy, Clone, Eq, PartialEq)]
            #type_def

            impl #impl_generics #type_name #ty_generics #where_clause {
                #constants

                /// Creates a new instance with reserved-as-1 bits set and
                /// all other bits zeroed (i.e., with a value of
                /// [`Self::RSVD1_MASK`]).
                pub const fn new() -> Self {
                    Self(Self::RSVD1_MASK)
                }

                #(#getters_and_setters)*
            }

            impl #impl_generics ::core::default::Default for #type_name #ty_generics #where_clause {
                /// Returns an instance with the default bits set (i.e,. with a
                /// value of [`Self::DEFAULT`].
                fn default() -> Self {
                    Self(Self::DEFAULT)
                }
            }

            impl #impl_generics ::core::convert::From<#base> for #type_name #ty_generics #where_clause {
                // `RSVD{0,1}_MASK` may be zero, in which case the following
                // mask conditions might be trivially true.
                #[allow(clippy::bad_bit_mask)]
                fn from(value: #base) -> Self {
                    debug_assert!(
                        value & Self::RSVD1_MASK == Self::RSVD1_MASK,
                        "from(): Invalid base value ({value:#x}) has reserved-as-1 bits ({:#x}) unset",
                        Self::RSVD1_MASK,
                    );
                    debug_assert!(
                        !value & Self::RSVD0_MASK == Self::RSVD0_MASK,
                        "from(): Invalid base value ({value:#x}) has reserved-as-0 bits ({:#x}) set",
                        Self::RSVD0_MASK,
                    );
                    Self(value)
                }
            }

            impl #impl_generics ::core::ops::Deref for #type_name #ty_generics #where_clause {
                type Target = #base;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl #impl_generics ::core::ops::DerefMut for #type_name #ty_generics #where_clause {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }

            #iter_impl

            #fmt_impls
        }
        .to_tokens(tokens);
    }
}
