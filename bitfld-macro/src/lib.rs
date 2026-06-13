// Copyright (c) 2025 Joshua Seaton
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT#

use proc_macro::TokenStream;
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Error, Parse, ParseStream, Result};
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, Fields, GenericArgument, Ident, ItemStruct, Lit,
    Pat, Path, PathArguments, Stmt, Type, braced, parse_macro_input,
    parse_quote,
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
    parse_macro_input!(item as Layout).to_token_stream().into()
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
}

impl BaseType {
    const fn high_bit(&self) -> usize {
        match *self {
            Self::U8 => 7,
            Self::U16 => 15,
            Self::U32 => 31,
            Self::U64 => 63,
            Self::U128 => 127,
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

        if !strct.generics.params.is_empty() {
            return Err(Error::new_spanned(
                &strct.generics,
                "generic parameters are not supported",
            ));
        }
        if let Some(where_clause) = &strct.generics.where_clause {
            return Err(Error::new_spanned(
                where_clause,
                "generic parameters are not supported",
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
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub const fn #name(&self) -> bool {
                    ::bitfld::get_bit!(self.0, #low_bit)
                }

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
            #getter
            #setter
        }
    }
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
            doc_attrs,
            unshifted,
            default: default_or_value,
        })
    }
}

struct Layout {
    ty: TypeDef,
    named: Vec<Bitfield>,
    reserved: Vec<Bitfield>,
}

impl Layout {
    fn constants(&self) -> TokenStream2 {
        let base = &self.ty.base.def;

        let mut field_constants = Vec::new();
        let mut field_metadata = Vec::new();
        let mut checks = Vec::new();
        let mut default_stmts = Vec::new();

        for field in &self.named {
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
                #[doc = #mask_doc]
                pub const #mask_name: #base = (#shifted_mask << #low_bit);
                #[doc = #shift_doc]
                pub const #shift_name: usize = #low_bit;
            });

            if let Some(default) = &field.default {
                let default_name = format_ident!("{name_upper}_DEFAULT");
                let doc = format!(
                    "Pre-shifted default value of the `{name_lower}` field.",
                );
                field_constants.push(quote! {
                    #[doc = #doc]
                    pub const #default_name: #base = ((#default) as #base) << #low_bit;
                });
                checks.push(quote! {
                    const { assert!(((#default) as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
                });
                default_stmts.push(quote! {
                    { v |= Self::#default_name; }
                });
            }

            let high_bit = Literal::usize_unsuffixed(field.high_bit);
            let default = if let Some(default) = &field.default {
                quote! { #default }
            } else {
                quote! { 0 }
            };
            field_metadata.push(quote! {
                ::bitfld::FieldMetadata::<#base>{
                    name: #name_lower,
                    high_bit: #high_bit,
                    low_bit: #low_bit,
                    default: #default as #base,
                },
            });
        }

        let num_fields = self.named.len();

        let mut rsvd1_stmts = Vec::new();
        let mut rsvd0_stmts = Vec::new();
        for rsvd in &self.reserved {
            let rsvd_value = rsvd.default.as_ref().unwrap();
            let high_bit = rsvd.high_bit;
            let low_bit = Literal::usize_unsuffixed(rsvd.low_bit);
            let shifted_mask =
                quote! {::bitfld::shifted_mask!(#base, #high_bit, #low_bit) };
            let name = format_ident!("RSVD_{}_{}", rsvd.high_bit, rsvd.low_bit);

            field_constants.push(quote! {
                const #name: #base = (#rsvd_value as #base) << #low_bit;
            });
            checks.push(quote! {
                const { assert!((#rsvd_value as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
            });
            rsvd1_stmts.push(quote! {
                { v |= Self::#name; }
            });
            rsvd0_stmts.push(quote! {
                { v |= !Self::#name & (#shifted_mask << #low_bit); }
            });
        }
        field_constants.push(quote! {
            #[doc(hidden)]
            const NUM_FIELDS: usize = #num_fields;
            /// Metadata of all named fields in the layout.
            pub const FIELDS: [::bitfld::FieldMetadata::<#base>; #num_fields] = [
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

        quote! {
            #[doc(hidden)]
            #vis struct #iter_type(#base, usize, usize);

            impl ::core::iter::Iterator for #iter_type {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);

                fn next(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    let metadata = &#ty::FIELDS[self.1];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    self.1 += 1;
                    Some((metadata, value))
                }
            }

            impl ::core::iter::DoubleEndedIterator for #iter_type {
                fn next_back(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    self.2 -= 1;
                    let metadata = &#ty::FIELDS[self.2];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    Some((metadata, value))
                }
            }

            impl #ty {
                /// Returns an iterator over
                /// ([metadata][`bitfld::FieldMetadata`], value) pairs for each
                /// field.
                pub fn iter(&self) -> #iter_type {
                    #iter_type(self.0, 0, Self::NUM_FIELDS)
                }
            }

            impl ::core::iter::IntoIterator for #ty {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, Self::NUM_FIELDS) }
            }

            impl<'a> ::core::iter::IntoIterator for &'a #ty {
                type Item = (&'static ::bitfld::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, #ty::NUM_FIELDS) }
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
        let lower_hex_fmt = self.fmt_fn(":#x");
        let upper_hex_fmt = self.fmt_fn(":#X");
        let binary_fmt = self.fmt_fn(":#b");
        let octal_fmt = self.fmt_fn(":#o");
        quote! {
            impl ::core::fmt::Debug for #ty {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    ::core::fmt::LowerHex::fmt(self, f)
                }
            }

            impl ::core::fmt::Binary for #ty {
                #binary_fmt
            }

            impl ::core::fmt::LowerHex for #ty {
                #lower_hex_fmt
            }

            impl ::core::fmt::UpperHex for #ty {
                #upper_hex_fmt
            }

            impl ::core::fmt::Octal for #ty {
                #octal_fmt
            }
        }
    }
}

/// The sequence of bitfield `let` items (no enclosing block). Pure syntactic
/// content — no validation against any base type, no sort, no named/reserved
/// split. That all happens at the `Layout` level (or, later, `Multilayout`).
/// Callers are responsible for peeling whatever braces the surrounding
/// grammar requires before handing the inner stream to `Bitfields::parse`.
struct Bitfields(Vec<Bitfield>);

impl Parse for Bitfields {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut fields = Vec::new();
        while !input.is_empty() {
            fields.push(input.parse::<Bitfield>()?);
        }
        Ok(Self(fields))
    }
}

impl Parse for Layout {
    fn parse(input: ParseStream) -> Result<Self> {
        let input = {
            let content;
            braced!(content in input);
            content
        };

        let ty = input.parse::<TypeDef>()?;

        let inner = {
            let content;
            braced!(content in input);
            content
        };
        let mut fields = inner.parse::<Bitfields>()?.0;

        fields.sort_by_key(|field| field.low_bit);

        for pair in fields.windows(2) {
            let (curr, next) = (&pair[0], &pair[1]);
            if curr.high_bit >= next.low_bit {
                // TODO(https://github.com/rust-lang/rust/issues/54725): It
                // would be nice to Span::join() the two spans, but that's
                // still experimental.
                return Err(Error::new(
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

        let highest_possible = ty.base.ty.high_bit();
        if let Some(highest) = fields.last()
            && highest.high_bit > highest_possible
        {
            return Err(Error::new(
                highest.span,
                format!(
                    "high bit {} exceeds the highest possible value \
                     of {highest_possible}",
                    highest.high_bit
                ),
            ));
        }

        let mut layout = Self {
            ty,
            named: vec![],
            reserved: vec![],
        };

        while let Some(field) = fields.pop() {
            if field.is_reserved() {
                if field.default.is_some() {
                    layout.reserved.push(field);
                }
            } else {
                layout.named.push(field);
            }
        }

        Ok(layout)
    }
}

impl ToTokens for Layout {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let type_def = &self.ty.def;
        let type_name = &type_def.ident;
        let base = &self.ty.base.def;

        let constants = self.constants();
        let getters_and_setters = self.getters_and_setters();
        let iter_impl = self.iter_impl();
        let fmt_impls = self.fmt_impls();
        quote! {
            #[derive(Copy, Clone, Eq, PartialEq)]
            #type_def

            impl #type_name {
                #constants

                /// Creates a new instance with reserved-as-1 bits set and
                /// all other bits zeroed (i.e., with a value of
                /// [`Self::RSVD1_MASK`]).
                pub const fn new() -> Self {
                    Self(Self::RSVD1_MASK)
                }

                #(#getters_and_setters)*
            }

            impl ::core::default::Default for #type_name {
                /// Returns an instance with the default bits set (i.e,. with a
                /// value of [`Self::DEFAULT`].
                fn default() -> Self {
                    Self(Self::DEFAULT)
                }
            }

            impl ::core::convert::From<#base> for #type_name {
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

            impl ::core::ops::Deref for #type_name {
                type Target = #base;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl ::core::ops::DerefMut for #type_name {
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
