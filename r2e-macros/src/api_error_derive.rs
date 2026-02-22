use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Fields, Ident, Lit, Meta, Type, Variant,
};

use crate::crate_path::r2e_core_path;

// ── Parsed types ─────────────────────────────────────────────────────────

struct ApiErrorDef {
    name: Ident,
    generics: syn::Generics,
    variants: Vec<ApiErrorVariant>,
}

struct ApiErrorVariant {
    ident: Ident,
    fields: VariantFields,
    error_attr: ErrorAttr,
}

enum VariantFields {
    Unit,
    Tuple(Vec<TupleField>),
    Named(Vec<NamedField>),
}

struct TupleField {
    ty: Type,
    is_from: bool,
}

struct NamedField {
    name: Ident,
    ty: Type,
    is_from: bool,
}

enum ErrorAttr {
    Standard {
        status: StatusExpr,
        message: Option<String>,
    },
    Transparent,
}

enum StatusExpr {
    Named(Ident),
    Numeric(u16),
}

// ── Entry point ──────────────────────────────────────────────────────────

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let generics = &input.generics;

    let variants = match &input.data {
        Data::Enum(data) => &data.variants,
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "ApiError can only be derived for enums",
            ))
        }
    };

    let parsed = parse_variants(variants)?;

    let def = ApiErrorDef {
        name: name.clone(),
        generics: generics.clone(),
        variants: parsed,
    };

    let krate = r2e_core_path();
    let display_impl = gen_display(&def);
    let into_response_impl = gen_into_response(&def, &krate);
    let error_impl = gen_error(&def);
    let from_impls = gen_from_impls(&def);

    let (impl_generics, ty_generics, where_clause) = def.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::core::fmt::Display for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #display_impl
            }
        }

        impl #impl_generics #krate::http::response::IntoResponse for #name #ty_generics #where_clause {
            fn into_response(self) -> #krate::http::response::Response {
                #into_response_impl
            }
        }

        impl #impl_generics ::std::error::Error for #name #ty_generics #where_clause {
            fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
                #error_impl
            }
        }

        #from_impls
    })
}

// ── Parsing ──────────────────────────────────────────────────────────────

fn parse_variants(
    variants: &syn::punctuated::Punctuated<Variant, syn::token::Comma>,
) -> syn::Result<Vec<ApiErrorVariant>> {
    variants.iter().map(parse_variant).collect()
}

fn parse_variant(variant: &Variant) -> syn::Result<ApiErrorVariant> {
    let ident = variant.ident.clone();

    let error_attr = parse_error_attr(&variant.attrs, &ident)?;
    let fields = parse_fields(&variant.fields)?;

    // Validate: transparent requires exactly one field
    if let ErrorAttr::Transparent = &error_attr {
        let field_count = match &fields {
            VariantFields::Unit => 0,
            VariantFields::Tuple(f) => f.len(),
            VariantFields::Named(f) => f.len(),
        };
        if field_count != 1 {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "#[error(transparent)] requires exactly one field",
            ));
        }
    }

    // Validate: at most one #[from] per variant
    let from_count = match &fields {
        VariantFields::Unit => 0,
        VariantFields::Tuple(f) => f.iter().filter(|f| f.is_from).count(),
        VariantFields::Named(f) => f.iter().filter(|f| f.is_from).count(),
    };
    if from_count > 1 {
        return Err(syn::Error::new_spanned(
            &variant.ident,
            "only one #[from] per variant",
        ));
    }

    Ok(ApiErrorVariant {
        ident,
        fields,
        error_attr,
    })
}

fn parse_error_attr(attrs: &[Attribute], variant_ident: &Ident) -> syn::Result<ErrorAttr> {
    let attr = attrs
        .iter()
        .find(|a| a.path().is_ident("error"))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                variant_ident,
                "each variant must have an #[error(...)] attribute",
            )
        })?;

    let nested = attr.parse_args_with(
        syn::punctuated::Punctuated::<Meta, syn::token::Comma>::parse_terminated,
    )?;

    // Check for #[error(transparent)]
    for meta in &nested {
        if let Meta::Path(p) = meta {
            if p.is_ident("transparent") {
                return Ok(ErrorAttr::Transparent);
            }
        }
    }

    let mut status: Option<StatusExpr> = None;
    let mut message: Option<String> = None;

    for meta in &nested {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("status") => {
                status = Some(parse_status_expr(&nv.value)?);
            }
            Meta::NameValue(nv) if nv.path.is_ident("message") => {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(s), ..
                }) = &nv.value
                {
                    message = Some(s.value());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.value,
                        "message must be a string literal",
                    ));
                }
            }
            _ => {}
        }
    }

    let status = status.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "#[error(...)] requires status = STATUS_CODE",
        )
    })?;

    Ok(ErrorAttr::Standard { status, message })
}

fn parse_status_expr(expr: &syn::Expr) -> syn::Result<StatusExpr> {
    match expr {
        syn::Expr::Path(p) => {
            if let Some(ident) = p.path.get_ident() {
                Ok(StatusExpr::Named(ident.clone()))
            } else {
                // Multi-segment path — extract the last segment as the ident
                if let Some(seg) = p.path.segments.last() {
                    Ok(StatusExpr::Named(seg.ident.clone()))
                } else {
                    Err(syn::Error::new_spanned(expr, "invalid status code"))
                }
            }
        }
        syn::Expr::Lit(syn::ExprLit {
            lit: Lit::Int(lit), ..
        }) => {
            let val: u16 = lit.base10_parse()?;
            Ok(StatusExpr::Numeric(val))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "status must be a StatusCode constant (e.g. NOT_FOUND) or a numeric literal (e.g. 429)",
        )),
    }
}

fn parse_fields(fields: &Fields) -> syn::Result<VariantFields> {
    match fields {
        Fields::Unit => Ok(VariantFields::Unit),
        Fields::Unnamed(unnamed) => {
            let parsed = unnamed
                .unnamed
                .iter()
                .map(|f| {
                    let is_from = f.attrs.iter().any(|a| a.path().is_ident("from"));
                    TupleField {
                        ty: f.ty.clone(),
                        is_from,
                    }
                })
                .collect();
            Ok(VariantFields::Tuple(parsed))
        }
        Fields::Named(named) => {
            let parsed = named
                .named
                .iter()
                .map(|f| {
                    let is_from = f.attrs.iter().any(|a| a.path().is_ident("from"));
                    NamedField {
                        name: f.ident.clone().unwrap(),
                        ty: f.ty.clone(),
                        is_from,
                    }
                })
                .collect();
            Ok(VariantFields::Named(parsed))
        }
    }
}

// ── Codegen: Display ─────────────────────────────────────────────────────

fn gen_display(def: &ApiErrorDef) -> TokenStream2 {
    let name = &def.name;
    let arms: Vec<TokenStream2> = def
        .variants
        .iter()
        .map(|v| gen_display_arm(name, v))
        .collect();

    quote! {
        match self {
            #(#arms)*
        }
    }
}

fn gen_display_arm(enum_name: &Ident, variant: &ApiErrorVariant) -> TokenStream2 {
    let vname = &variant.ident;

    match &variant.error_attr {
        ErrorAttr::Transparent => {
            // Delegate to inner Display
            let (pattern, inner_expr) = single_field_pattern(enum_name, variant, false);
            quote! {
                #pattern => ::core::fmt::Display::fmt(#inner_expr, f),
            }
        }
        ErrorAttr::Standard { message, .. } => {
            match message {
                Some(msg) => {
                    // Explicit message with interpolation
                    let (pattern, fmt_str) =
                        interpolated_message_pattern(enum_name, variant, msg);
                    quote! {
                        #pattern => write!(f, #fmt_str),
                    }
                }
                None => {
                    // Infer message
                    match &variant.fields {
                        VariantFields::Unit => {
                            let humanized = humanize_ident(vname);
                            quote! {
                                #enum_name::#vname => write!(f, #humanized),
                            }
                        }
                        VariantFields::Tuple(fields) => {
                            let from_field = fields.iter().position(|f| f.is_from);
                            if let Some(idx) = from_field {
                                // Use source.to_string()
                                let bindings: Vec<TokenStream2> = fields
                                    .iter()
                                    .enumerate()
                                    .map(|(i, _)| {
                                        let id = format_ident!("_{}", i);
                                        quote!(#id)
                                    })
                                    .collect();
                                let src = format_ident!("_{}", idx);
                                quote! {
                                    #enum_name::#vname(#(#bindings),*) => write!(f, "{}", #src),
                                }
                            } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                                // Single String field → use field value
                                quote! {
                                    #enum_name::#vname(ref _0) => write!(f, "{}", _0),
                                }
                            } else {
                                let humanized = humanize_ident(vname);
                                let bindings: Vec<TokenStream2> = fields
                                    .iter()
                                    .enumerate()
                                    .map(|(i, _)| {
                                        let id = format_ident!("_{}", i);
                                        quote!(#id)
                                    })
                                    .collect();
                                quote! {
                                    #enum_name::#vname(#(#bindings),*) => write!(f, #humanized),
                                }
                            }
                        }
                        VariantFields::Named(fields) => {
                            let from_field = fields.iter().find(|f| f.is_from);
                            let field_names: Vec<&Ident> =
                                fields.iter().map(|f| &f.name).collect();
                            if let Some(ff) = from_field {
                                let src_name = &ff.name;
                                quote! {
                                    #enum_name::#vname { #(ref #field_names),* } => write!(f, "{}", #src_name),
                                }
                            } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                                let fname = &fields[0].name;
                                quote! {
                                    #enum_name::#vname { ref #fname } => write!(f, "{}", #fname),
                                }
                            } else {
                                let humanized = humanize_ident(vname);
                                quote! {
                                    #enum_name::#vname { .. } => write!(f, #humanized),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Codegen: IntoResponse ────────────────────────────────────────────────

fn gen_into_response(def: &ApiErrorDef, krate: &TokenStream2) -> TokenStream2 {
    let name = &def.name;
    let arms: Vec<TokenStream2> = def
        .variants
        .iter()
        .map(|v| gen_response_arm(name, v, krate))
        .collect();

    quote! {
        match self {
            #(#arms)*
        }
    }
}

fn gen_response_arm(
    enum_name: &Ident,
    variant: &ApiErrorVariant,
    krate: &TokenStream2,
) -> TokenStream2 {
    match &variant.error_attr {
        ErrorAttr::Transparent => {
            let (pattern, inner_expr) = single_field_pattern(enum_name, variant, true);
            quote! {
                #pattern => #krate::http::response::IntoResponse::into_response(#inner_expr),
            }
        }
        ErrorAttr::Standard { status, message } => {
            let status_tokens = status_to_tokens(status, krate);
            gen_response_arm_with_pattern(
                enum_name,
                variant,
                &status_tokens,
                krate,
                message.as_deref(),
            )
        }
    }
}

fn gen_response_arm_with_pattern(
    enum_name: &Ident,
    variant: &ApiErrorVariant,
    status_tokens: &TokenStream2,
    krate: &TokenStream2,
    message: Option<&str>,
) -> TokenStream2 {
    let vname = &variant.ident;

    match message {
        Some(msg) => {
            let (pattern, fmt_str) = interpolated_message_pattern(enum_name, variant, msg);
            quote! {
                #pattern => {
                    let __msg = format!(#fmt_str);
                    #krate::error::error_response(#status_tokens, __msg)
                }
            }
        }
        None => {
            // Infer message — same rules as Display
            match &variant.fields {
                VariantFields::Unit => {
                    let humanized = humanize_ident(vname);
                    quote! {
                        #enum_name::#vname => {
                            #krate::error::error_response(#status_tokens, #humanized)
                        }
                    }
                }
                VariantFields::Tuple(fields) => {
                    let from_field = fields.iter().position(|f| f.is_from);
                    if let Some(idx) = from_field {
                        let bindings: Vec<TokenStream2> = fields
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                let id = format_ident!("_{}", i);
                                quote!(#id)
                            })
                            .collect();
                        let src = format_ident!("_{}", idx);
                        quote! {
                            #enum_name::#vname(#(#bindings),*) => {
                                #krate::error::error_response(#status_tokens, #src.to_string())
                            }
                        }
                    } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                        quote! {
                            #enum_name::#vname(ref _0) => {
                                #krate::error::error_response(#status_tokens, _0.clone())
                            }
                        }
                    } else {
                        let humanized = humanize_ident(vname);
                        let bindings: Vec<TokenStream2> = fields
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                let id = format_ident!("_{}", i);
                                quote!(#id)
                            })
                            .collect();
                        quote! {
                            #enum_name::#vname(#(#bindings),*) => {
                                #krate::error::error_response(#status_tokens, #humanized)
                            }
                        }
                    }
                }
                VariantFields::Named(fields) => {
                    let from_field = fields.iter().find(|f| f.is_from);
                    let field_names: Vec<&Ident> = fields.iter().map(|f| &f.name).collect();
                    if let Some(ff) = from_field {
                        let src_name = &ff.name;
                        quote! {
                            #enum_name::#vname { #(ref #field_names),* } => {
                                #krate::error::error_response(#status_tokens, #src_name.to_string())
                            }
                        }
                    } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                        let fname = &fields[0].name;
                        quote! {
                            #enum_name::#vname { ref #fname } => {
                                #krate::error::error_response(#status_tokens, #fname.clone())
                            }
                        }
                    } else {
                        let humanized = humanize_ident(vname);
                        quote! {
                            #enum_name::#vname { .. } => {
                                #krate::error::error_response(#status_tokens, #humanized)
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Codegen: std::error::Error ───────────────────────────────────────────

fn gen_error(def: &ApiErrorDef) -> TokenStream2 {
    let name = &def.name;
    // Only return source for non-transparent #[from] variants.
    // Transparent variants delegate Display + IntoResponse but don't
    // require the inner type to implement std::error::Error.
    let has_any_non_transparent_from = def.variants.iter().any(|v| {
        !matches!(v.error_attr, ErrorAttr::Transparent) && variant_has_from(v)
    });

    if !has_any_non_transparent_from {
        return quote! { None };
    }

    let arms: Vec<TokenStream2> = def
        .variants
        .iter()
        .map(|v| {
            let vname = &v.ident;
            let is_transparent = matches!(v.error_attr, ErrorAttr::Transparent);
            if !is_transparent {
                if let Some((pattern, source_expr)) = from_source_pattern(name, v) {
                    return quote! {
                        #pattern => Some(#source_expr as &(dyn ::std::error::Error + 'static)),
                    };
                }
            }
            // Wildcard arm for variants without #[from] or transparent variants
            match &v.fields {
                VariantFields::Unit => quote! { #name::#vname => None, },
                VariantFields::Tuple(_) => quote! { #name::#vname(..) => None, },
                VariantFields::Named(_) => quote! { #name::#vname { .. } => None, },
            }
        })
        .collect();

    quote! {
        match self {
            #(#arms)*
        }
    }
}

// ── Codegen: From impls ──────────────────────────────────────────────────

fn gen_from_impls(def: &ApiErrorDef) -> TokenStream2 {
    let name = &def.name;
    let (impl_generics, ty_generics, where_clause) = def.generics.split_for_impl();

    let impls: Vec<TokenStream2> = def
        .variants
        .iter()
        .filter_map(|v| {
            let vname = &v.ident;
            match &v.fields {
                VariantFields::Tuple(fields) => {
                    let from_idx = fields.iter().position(|f| f.is_from)?;
                    let from_ty = &fields[from_idx].ty;
                    let field_count = fields.len();

                    let args: Vec<TokenStream2> = (0..field_count)
                        .map(|i| {
                            if i == from_idx {
                                quote!(source)
                            } else {
                                quote!(Default::default())
                            }
                        })
                        .collect();

                    Some(quote! {
                        impl #impl_generics ::core::convert::From<#from_ty> for #name #ty_generics #where_clause {
                            fn from(source: #from_ty) -> Self {
                                #name::#vname(#(#args),*)
                            }
                        }
                    })
                }
                VariantFields::Named(fields) => {
                    let from_field = fields.iter().find(|f| f.is_from)?;
                    let from_ty = &from_field.ty;

                    let field_inits: Vec<TokenStream2> = fields
                        .iter()
                        .map(|f| {
                            let fname = &f.name;
                            if f.is_from {
                                quote!(#fname: source)
                            } else {
                                quote!(#fname: Default::default())
                            }
                        })
                        .collect();

                    Some(quote! {
                        impl #impl_generics ::core::convert::From<#from_ty> for #name #ty_generics #where_clause {
                            fn from(source: #from_ty) -> Self {
                                #name::#vname { #(#field_inits),* }
                            }
                        }
                    })
                }
                VariantFields::Unit => None,
            }
        })
        .collect();

    quote! { #(#impls)* }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn status_to_tokens(status: &StatusExpr, krate: &TokenStream2) -> TokenStream2 {
    match status {
        StatusExpr::Named(ident) => {
            quote! { #krate::http::StatusCode::#ident }
        }
        StatusExpr::Numeric(code) => {
            quote! { #krate::http::StatusCode::from_u16(#code).unwrap() }
        }
    }
}

fn humanize_ident(ident: &Ident) -> String {
    let s = ident.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push(' ');
            result.push(ch.to_lowercase().next().unwrap());
        } else if i == 0 {
            result.push(ch); // Keep first char as-is (uppercase)
        } else {
            result.push(ch);
        }
    }
    result
}

fn is_string_type(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == "String";
        }
    }
    false
}

/// Returns (match_pattern, inner_value_expr) for a single-field variant.
/// When `owned` is true, generates move bindings (for IntoResponse which takes `self`).
/// When false, generates `ref` bindings (for Display/source which take `&self`).
fn single_field_pattern(enum_name: &Ident, variant: &ApiErrorVariant, owned: bool) -> (TokenStream2, TokenStream2) {
    let vname = &variant.ident;
    match &variant.fields {
        VariantFields::Tuple(_) => {
            if owned {
                (quote!(#enum_name::#vname(__inner)), quote!(__inner))
            } else {
                (quote!(#enum_name::#vname(ref __inner)), quote!(__inner))
            }
        }
        VariantFields::Named(fields) => {
            let fname = &fields[0].name;
            if owned {
                (quote!(#enum_name::#vname { #fname }), quote!(#fname))
            } else {
                (quote!(#enum_name::#vname { ref #fname }), quote!(#fname))
            }
        }
        VariantFields::Unit => unreachable!("transparent requires one field"),
    }
}

/// For variants with `#[from]`, returns (match_pattern, source_ref_expr).
fn from_source_pattern(
    enum_name: &Ident,
    variant: &ApiErrorVariant,
) -> Option<(TokenStream2, TokenStream2)> {
    let vname = &variant.ident;
    match &variant.fields {
        VariantFields::Tuple(fields) => {
            let from_idx = fields.iter().position(|f| f.is_from)?;
            let bindings: Vec<TokenStream2> = fields
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let id = format_ident!("_{}", i);
                    quote!(ref #id)
                })
                .collect();
            let src = format_ident!("_{}", from_idx);
            Some((
                quote!(#enum_name::#vname(#(#bindings),*)),
                quote!(#src),
            ))
        }
        VariantFields::Named(fields) => {
            let from_field = fields.iter().find(|f| f.is_from)?;
            let from_name = &from_field.name;
            let field_names: Vec<TokenStream2> = fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote!(ref #n)
                })
                .collect();
            Some((
                quote!(#enum_name::#vname { #(#field_names),* }),
                quote!(#from_name),
            ))
        }
        VariantFields::Unit => None,
    }
}

fn variant_has_from(variant: &ApiErrorVariant) -> bool {
    match &variant.fields {
        VariantFields::Tuple(fields) => fields.iter().any(|f| f.is_from),
        VariantFields::Named(fields) => fields.iter().any(|f| f.is_from),
        VariantFields::Unit => false,
    }
}

/// Builds (match_pattern, format_string) for a message with `{0}` / `{field}` interpolation.
///
/// The format string uses captured identifier syntax so `format!()` works directly.
/// For tuple fields: `{0}` → binding `_0`, format string `{_0}`.
/// For named fields: `{field}` → binding `field`, format string `{field}`.
fn interpolated_message_pattern(
    enum_name: &Ident,
    variant: &ApiErrorVariant,
    message: &str,
) -> (TokenStream2, String) {
    let vname = &variant.ident;

    match &variant.fields {
        VariantFields::Unit => (
            quote!(#enum_name::#vname),
            message.to_string(),
        ),
        VariantFields::Tuple(fields) => {
            let bindings: Vec<TokenStream2> = fields
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let id = format_ident!("_{}", i);
                    quote!(ref #id)
                })
                .collect();

            // Replace {0}, {1}, etc. with {_0}, {_1} for format!() captured idents
            let mut fmt_str = message.to_string();
            for i in (0..fields.len()).rev() {
                fmt_str = fmt_str.replace(
                    &format!("{{{}}}", i),
                    &format!("{{_{}}}", i),
                );
            }

            (
                quote!(#enum_name::#vname(#(#bindings),*)),
                fmt_str,
            )
        }
        VariantFields::Named(fields) => {
            let field_names: Vec<TokenStream2> = fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote!(ref #n)
                })
                .collect();

            // Named fields: {field} already works with format!() captured idents
            (
                quote!(#enum_name::#vname { #(#field_names),* }),
                message.to_string(),
            )
        }
    }
}
