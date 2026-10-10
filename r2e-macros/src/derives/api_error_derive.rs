use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Fields, Ident, Lit, Meta, Type, Variant,
};

use crate::util::crate_path::r2e_core_path;

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
    /// `#[error(rejection)]` (or `#[error(transparent)]` over a `Rejection`
    /// field): the variant carries the framework-side failure, so the enum
    /// gets `From<Rejection>` + `ErrorSchema` and can be a route's envelope.
    Rejection,
}

enum StatusExpr {
    /// Bare ident like `NOT_FOUND` — prepended with `StatusCode::` when emitted.
    BareIdent(Ident),
    /// Qualified path like `http::StatusCode::NOT_FOUND` — emitted verbatim so
    /// typos or unknown constants fail at the user's own span.
    Qualified(syn::Path),
    /// Numeric literal, bounds-checked to `100..=599` at parse time.
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
            ));
        }
    };

    let parsed = parse_variants(variants)?;

    let def = ApiErrorDef {
        name: name.clone(),
        generics: generics.clone(),
        variants: parsed,
    };

    validate_rejection_variants(&def)?;

    let krate = r2e_core_path();
    let display_impl = gen_display(&def);
    let into_response_impl = gen_into_response(&def, &krate);
    let error_impl = gen_error(&def);
    let from_impls = gen_from_impls(&def);
    let projection_impls = gen_projection_impls(&def, &krate);

    let (impl_generics, ty_generics, where_clause) = def.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::core::fmt::Display for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #display_impl
            }
        }

        impl #impl_generics #krate::http::response::IntoHttpResponse for #name #ty_generics #where_clause {
            fn into_http_response(self) -> #krate::http::response::Response {
                #into_response_impl
            }
        }

        // Bridge to the HTTP backend's response contract. `impl_into_response!`
        // cannot be used here (the type may be generic), so the derive emits
        // the delegation itself — it is the second and last named bridge point
        // on the response side (plan §5.3b).
        impl #impl_generics #krate::http::response::IntoResponse for #name #ty_generics #where_clause {
            fn into_response(self) -> #krate::http::response::Response {
                <Self as #krate::http::response::IntoHttpResponse>::into_http_response(self)
            }
        }

        impl #impl_generics ::std::error::Error for #name #ty_generics #where_clause {
            fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
                #error_impl
            }
        }

        #from_impls

        #projection_impls
    })
}

/// At most one variant may carry the `Rejection`.
fn validate_rejection_variants(def: &ApiErrorDef) -> syn::Result<()> {
    let mut seen: Option<&Ident> = None;
    for v in &def.variants {
        if matches!(v.error_attr, ErrorAttr::Rejection) {
            if let Some(first) = seen {
                return Err(syn::Error::new_spanned(
                    &v.ident,
                    format!(
                        "only one variant may carry the `Rejection` (`{first}` already does): \
                         `From<Rejection>` would be ambiguous"
                    ),
                ));
            }
            seen = Some(&v.ident);
        }
    }
    Ok(())
}

// ── Parsing ──────────────────────────────────────────────────────────────

fn parse_variants(
    variants: &syn::punctuated::Punctuated<Variant, syn::token::Comma>,
) -> syn::Result<Vec<ApiErrorVariant>> {
    variants.iter().map(parse_variant).collect()
}

fn parse_variant(variant: &Variant) -> syn::Result<ApiErrorVariant> {
    let ident = variant.ident.clone();

    let mut error_attr = parse_error_attr(&variant.attrs, &ident)?;
    let fields = parse_fields(&variant.fields)?;

    let field_count = match &fields {
        VariantFields::Unit => 0,
        VariantFields::Tuple(f) => f.len(),
        VariantFields::Named(f) => f.len(),
    };

    // Validate: transparent requires exactly one field
    if let ErrorAttr::Transparent = &error_attr {
        if field_count != 1 {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "#[error(transparent)] requires exactly one field",
            ));
        }
        // `#[error(transparent)]` over a `Rejection` field is the rejection
        // variant: same Display/response delegation, plus `From<Rejection>`.
        if single_field_type(&fields).is_some_and(|ty| type_last_segment_is(ty, "Rejection")) {
            error_attr = ErrorAttr::Rejection;
        }
    }

    // Validate: rejection requires exactly one field
    if let ErrorAttr::Rejection = &error_attr {
        if field_count != 1 {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "#[error(rejection)] requires exactly one field, of type `Rejection`",
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

    // Check for #[error(transparent)] / #[error(rejection)]
    let mut is_rejection = false;
    for meta in &nested {
        if let Meta::Path(p) = meta {
            if p.is_ident("transparent") {
                return Ok(ErrorAttr::Transparent);
            }
            if p.is_ident("rejection") {
                is_rejection = true;
            }
        }
    }
    if is_rejection {
        for meta in &nested {
            if let Meta::NameValue(nv) = meta {
                if nv.path.is_ident("status") || nv.path.is_ident("message") {
                    return Err(syn::Error::new_spanned(
                        nv,
                        "#[error(rejection)] takes no `status`/`message`: both come from the \
                         carried `Rejection` (remap statuses via `ErrorSchema::status_of`)",
                    ));
                }
            }
        }
        return Ok(ErrorAttr::Rejection);
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
        syn::Error::new_spanned(attr, "#[error(...)] requires status = STATUS_CODE")
    })?;

    Ok(ErrorAttr::Standard { status, message })
}

fn parse_status_expr(expr: &syn::Expr) -> syn::Result<StatusExpr> {
    match expr {
        syn::Expr::Path(p) => {
            if let Some(ident) = p.path.get_ident() {
                Ok(StatusExpr::BareIdent(ident.clone()))
            } else if p.path.segments.is_empty() {
                Err(syn::Error::new_spanned(expr, "invalid status code"))
            } else {
                // Keep the full path — typos will surface at the user's span
                // instead of being silently remapped to `StatusCode::<last>`.
                Ok(StatusExpr::Qualified(p.path.clone()))
            }
        }
        syn::Expr::Lit(syn::ExprLit {
            lit: Lit::Int(lit), ..
        }) => {
            let val: u16 = lit.base10_parse()?;
            if !(100..=599).contains(&val) {
                return Err(syn::Error::new_spanned(
                    lit,
                    format!("invalid HTTP status code {val}: must be between 100 and 599"),
                ));
            }
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
        ErrorAttr::Transparent | ErrorAttr::Rejection => {
            // Delegate to inner Display (a `Rejection` displays its message)
            let (pattern, inner_expr) = single_field_pattern(enum_name, variant);
            quote! {
                #pattern => ::core::fmt::Display::fmt(#inner_expr, f),
            }
        }
        ErrorAttr::Standard { message, .. } => {
            match message {
                Some(msg) => {
                    // Explicit message with interpolation
                    let (pattern, fmt_str) = interpolated_message_pattern(enum_name, variant, msg);
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
                                    #enum_name::#vname(_0) => write!(f, "{}", _0),
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
                            let field_names: Vec<&Ident> = fields.iter().map(|f| &f.name).collect();
                            if let Some(ff) = from_field {
                                let src_name = &ff.name;
                                quote! {
                                    #enum_name::#vname { #(#field_names),* } => write!(f, "{}", #src_name),
                                }
                            } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                                let fname = &fields[0].name;
                                quote! {
                                    #enum_name::#vname { #fname } => write!(f, "{}", #fname),
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
            let (pattern, inner_expr) = single_field_pattern(enum_name, variant);
            quote! {
                #pattern => #krate::http::response::IntoResponse::into_response(#inner_expr),
            }
        }
        ErrorAttr::Rejection => {
            // Default projection: `HttpError` bodies + the hub's headers.
            let (pattern, inner_expr) = single_field_pattern(enum_name, variant);
            quote! {
                #pattern => #krate::http::response::IntoHttpResponse::into_http_response(#inner_expr),
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
                            #enum_name::#vname(_0) => {
                                #krate::error::error_response(#status_tokens, _0)
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
                            #enum_name::#vname { #(#field_names),* } => {
                                #krate::error::error_response(#status_tokens, #src_name.to_string())
                            }
                        }
                    } else if fields.len() == 1 && is_string_type(&fields[0].ty) {
                        let fname = &fields[0].name;
                        quote! {
                            #enum_name::#vname { #fname } => {
                                #krate::error::error_response(#status_tokens, #fname)
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
    // A `Rejection` variant always has a source (`Rejection: Error`).
    let has_any_source = def.variants.iter().any(|v| match v.error_attr {
        ErrorAttr::Transparent => false,
        ErrorAttr::Rejection => true,
        ErrorAttr::Standard { .. } => variant_has_from(v),
    });

    if !has_any_source {
        return quote! { None };
    }

    let arms: Vec<TokenStream2> = def
        .variants
        .iter()
        .map(|v| {
            let vname = &v.ident;
            match v.error_attr {
                ErrorAttr::Rejection => {
                    let (pattern, inner_expr) = single_field_pattern(name, v);
                    return quote! {
                        #pattern => Some(#inner_expr as &(dyn ::std::error::Error + 'static)),
                    };
                }
                ErrorAttr::Standard { .. } => {
                    if let Some((pattern, source_expr)) = from_source_pattern(name, v) {
                        return quote! {
                            #pattern => Some(#source_expr as &(dyn ::std::error::Error + 'static)),
                        };
                    }
                }
                ErrorAttr::Transparent => {}
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
        .filter(|v| !matches!(v.error_attr, ErrorAttr::Rejection))
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

// ── Codegen: From<Rejection> + ErrorSchema ───────────────────────────────

/// How the enum receives a framework-side failure, if it can.
enum ProjectionSource<'a> {
    /// `#[error(rejection)]` variant: wraps the `Rejection` itself.
    Rejection(&'a ApiErrorVariant),
    /// Exactly one `#[error(transparent)]` variant over `HttpError`: the
    /// enum inherits `HttpError`'s projection (`HttpError: From<Rejection>`).
    HttpError(&'a ApiErrorVariant),
}

fn projection_source(def: &ApiErrorDef) -> Option<ProjectionSource<'_>> {
    if let Some(v) = def
        .variants
        .iter()
        .find(|v| matches!(v.error_attr, ErrorAttr::Rejection))
    {
        return Some(ProjectionSource::Rejection(v));
    }
    let mut over_http_error = def.variants.iter().filter(|v| {
        matches!(v.error_attr, ErrorAttr::Transparent)
            && single_field_type(&v.fields).is_some_and(|ty| type_last_segment_is(ty, "HttpError"))
    });
    let first = over_http_error.next()?;
    // Two transparent `HttpError` variants: no unambiguous `From<Rejection>`.
    if over_http_error.next().is_some() {
        return None;
    }
    Some(ProjectionSource::HttpError(first))
}

/// Emits `From<Rejection>` + `ErrorSchema` when the enum can receive a
/// framework-side failure. Bodies and statuses are `HttpError`'s (the
/// default projection); `extra_statuses` lists every fixed-status variant so
/// the OpenAPI builder documents the whole envelope.
fn gen_projection_impls(def: &ApiErrorDef, krate: &TokenStream2) -> TokenStream2 {
    let Some(source) = projection_source(def) else {
        return quote! {};
    };
    let name = &def.name;
    let (impl_generics, ty_generics, where_clause) = def.generics.split_for_impl();

    let (variant, wrap) = match source {
        ProjectionSource::Rejection(v) => (v, quote!(rejection)),
        ProjectionSource::HttpError(v) => (
            v,
            quote!(<#krate::error::HttpError as ::core::convert::From<#krate::error::Rejection>>::from(rejection)),
        ),
    };
    let vname = &variant.ident;
    let construct = match &variant.fields {
        VariantFields::Tuple(_) => quote!(#name::#vname(#wrap)),
        VariantFields::Named(fields) => {
            let fname = &fields[0].name;
            quote!(#name::#vname { #fname: #wrap })
        }
        VariantFields::Unit => unreachable!("projection variants carry one field"),
    };

    let extra: Vec<TokenStream2> = def
        .variants
        .iter()
        .filter_map(|v| match &v.error_attr {
            ErrorAttr::Standard { status, .. } => {
                let status_tokens = status_to_tokens(status, krate);
                let description = humanize_ident(&v.ident);
                Some(quote! { (#status_tokens, #description) })
            }
            ErrorAttr::Transparent | ErrorAttr::Rejection => None,
        })
        .collect();

    quote! {
        impl #impl_generics ::core::convert::From<#krate::error::Rejection> for #name #ty_generics #where_clause {
            fn from(rejection: #krate::error::Rejection) -> Self {
                #construct
            }
        }

        impl #impl_generics #krate::error::ErrorSchema for #name #ty_generics #where_clause {
            fn body_schema() -> Option<(String, #krate::serde_json::Value)> {
                <#krate::error::HttpError as #krate::error::ErrorSchema>::body_schema()
            }

            fn body_schema_for(
                kind: #krate::error::RejectionKind,
            ) -> Option<(String, #krate::serde_json::Value)> {
                <#krate::error::HttpError as #krate::error::ErrorSchema>::body_schema_for(kind)
            }

            fn extra_statuses() -> Vec<(#krate::http::StatusCode, &'static str)> {
                let mut statuses: Vec<(#krate::http::StatusCode, &'static str)> = vec![#(#extra),*];
                statuses.sort_by_key(|(status, _)| status.as_u16());
                statuses.dedup_by_key(|(status, _)| status.as_u16());
                statuses
            }

            fn opaque_passthrough() -> bool {
                true
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn single_field_type(fields: &VariantFields) -> Option<&Type> {
    match fields {
        VariantFields::Tuple(f) if f.len() == 1 => Some(&f[0].ty),
        VariantFields::Named(f) if f.len() == 1 => Some(&f[0].ty),
        _ => None,
    }
}

fn type_last_segment_is(ty: &Type, name: &str) -> bool {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == name;
        }
    }
    false
}

fn status_to_tokens(status: &StatusExpr, krate: &TokenStream2) -> TokenStream2 {
    match status {
        StatusExpr::BareIdent(ident) => {
            quote! { #krate::http::StatusCode::#ident }
        }
        StatusExpr::Qualified(path) => {
            quote! { #path }
        }
        StatusExpr::Numeric(code) => {
            quote! {
                #krate::http::StatusCode::from_u16(#code)
                    .expect("r2e_macros: HTTP status validated at macro expansion")
            }
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
///
/// The same pattern serves both scrutinees. `Display`/`source` match on `&self`,
/// where match ergonomics binds by reference on their own — writing `ref` there
/// is an error as of edition 2024. `IntoResponse` matches on an owned `self`,
/// where the binding moves. Neither site needs the distinction spelled out.
fn single_field_pattern(
    enum_name: &Ident,
    variant: &ApiErrorVariant,
) -> (TokenStream2, TokenStream2) {
    let vname = &variant.ident;
    match &variant.fields {
        VariantFields::Tuple(_) => (quote!(#enum_name::#vname(__inner)), quote!(__inner)),
        VariantFields::Named(fields) => {
            let fname = &fields[0].name;
            (quote!(#enum_name::#vname { #fname }), quote!(#fname))
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
                    quote!(#id)
                })
                .collect();
            let src = format_ident!("_{}", from_idx);
            Some((quote!(#enum_name::#vname(#(#bindings),*)), quote!(#src)))
        }
        VariantFields::Named(fields) => {
            let from_field = fields.iter().find(|f| f.is_from)?;
            let from_name = &from_field.name;
            let field_names: Vec<TokenStream2> = fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote!(#n)
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
        VariantFields::Unit => (quote!(#enum_name::#vname), message.to_string()),
        VariantFields::Tuple(fields) => {
            let bindings: Vec<TokenStream2> = fields
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let id = format_ident!("_{}", i);
                    quote!(#id)
                })
                .collect();

            // Replace {0}, {1}, etc. with {_0}, {_1} for format!() captured idents
            let mut fmt_str = message.to_string();
            for i in (0..fields.len()).rev() {
                fmt_str = fmt_str.replace(&format!("{{{}}}", i), &format!("{{_{}}}", i));
            }

            (quote!(#enum_name::#vname(#(#bindings),*)), fmt_str)
        }
        VariantFields::Named(fields) => {
            let field_names: Vec<TokenStream2> = fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote!(#n)
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
