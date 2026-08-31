use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Ident, LitStr, Type};

use crate::util::crate_path::r2e_core_path;

enum ParamSource {
    Path { name: String },
    Query { name: String },
    Header { name: String },
}

#[derive(Clone)]
enum DefaultValue {
    Trait,      // #[param(default)] → Default::default()
    Expr(Expr), // #[param(default = 42)] → 42
}

struct ParamField {
    ident: Ident,
    ty: Type,
    source: ParamSource,
    is_optional: bool,
    default_value: Option<DefaultValue>,
}

struct ParamOptions {
    source: Option<ParamSource>,
    default_value: Option<DefaultValue>,
}

enum NestedMode {
    Flatten,        // #[params] — pass through parent prefix
    Prefix(String), // #[params(prefix)] or #[params(prefix = "custom")]
}

struct NestedParamsField {
    ident: Ident,
    ty: Type,
    mode: NestedMode,
}

/// Represents all parsed fields from the struct.
enum ParsedField {
    Param(ParamField),
    Nested(NestedParamsField),
    /// `#[serde(skip)]` / `#[serde(skip_deserializing)]`: never read from the
    /// request, filled with `Default::default()` and absent from the OpenAPI
    /// parameter list.
    Skipped(Ident),
}

// ── serde attribute parity ───────────────────────────────────────────────
//
// `#[derive(Params)]` reads the `#[serde(...)]` attributes a struct already
// carries instead of inventing a competing `#[params(rename…)]` spelling, so
// a DTO shipped behind `Query<T>` migrates to `Params` untouched.

/// A `#[serde(rename_all = "…")]` case convention.
#[derive(Clone, Copy)]
enum RenameAll {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameAll {
    fn parse(s: &str, span: proc_macro2::Span) -> syn::Result<Self> {
        Ok(match s {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            other => {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "unsupported #[serde(rename_all = \"{other}\")] for #[derive(Params)] — \
                         expected one of lowercase, UPPERCASE, PascalCase, camelCase, \
                         snake_case, SCREAMING_SNAKE_CASE, kebab-case, SCREAMING-KEBAB-CASE"
                    ),
                ))
            }
        })
    }

    /// Apply the convention to a `snake_case` Rust field name.
    fn apply(self, field: &str) -> String {
        let words: Vec<&str> = field.split('_').filter(|w| !w.is_empty()).collect();
        match self {
            Self::Lower => field.replace('_', ""),
            Self::Upper => field.replace('_', "").to_uppercase(),
            Self::Pascal => words.iter().map(|w| capitalize(w)).collect(),
            Self::Camel => words
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    if i == 0 {
                        w.to_string()
                    } else {
                        capitalize(w)
                    }
                })
                .collect(),
            Self::Snake => field.to_string(),
            Self::ScreamingSnake => field.to_uppercase(),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.replace('_', "-").to_uppercase(),
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Read the struct-level `#[serde(rename_all = "…")]`, if any.
///
/// Both `rename_all = "…"` and the per-direction
/// `rename_all(deserialize = "…")` form are honored — extraction is the
/// deserialize direction. Every other serde key is ignored, not rejected: a
/// `Params` struct may keep `Serialize` attributes it needs elsewhere.
fn parse_serde_rename_all(attrs: &[syn::Attribute]) -> syn::Result<Option<RenameAll>> {
    let mut found = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                if meta.input.peek(syn::Token![=]) {
                    let lit: LitStr = meta.value()?.parse()?;
                    found = Some((lit.value(), lit.span()));
                } else if meta.input.peek(syn::token::Paren) {
                    // rename_all(serialize = "…", deserialize = "…")
                    meta.parse_nested_meta(|inner| {
                        if inner.path.is_ident("deserialize") {
                            let lit: LitStr = inner.value()?.parse()?;
                            found = Some((lit.value(), lit.span()));
                        } else if inner.input.peek(syn::Token![=]) {
                            let _: LitStr = inner.value()?.parse()?;
                        }
                        Ok(())
                    })?;
                }
                return Ok(());
            }
            skip_meta_value(&meta)
        });
    }
    found
        .map(|(value, span)| RenameAll::parse(&value, span))
        .transpose()
}

/// What the field-level `#[serde(...)]` attributes say about extraction.
#[derive(Default)]
struct SerdeFieldAttrs {
    rename: Option<String>,
    skip: bool,
    /// `#[serde(flatten)]` — the nested struct's own keys are read from the
    /// same request, exactly like a bare `#[params]`.
    flatten: bool,
    /// `#[serde(default)]` / `#[serde(default = "path::to::fn")]`, mapped onto
    /// the same machinery as `#[param(default)]`.
    default_value: Option<DefaultValue>,
}

fn parse_serde_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<SerdeFieldAttrs> {
    let mut out = SerdeFieldAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if meta.input.peek(syn::Token![=]) {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename = Some(lit.value());
                } else if meta.input.peek(syn::token::Paren) {
                    meta.parse_nested_meta(|inner| {
                        if inner.path.is_ident("deserialize") {
                            let lit: LitStr = inner.value()?.parse()?;
                            out.rename = Some(lit.value());
                        } else if inner.input.peek(syn::Token![=]) {
                            let _: LitStr = inner.value()?.parse()?;
                        }
                        Ok(())
                    })?;
                }
                return Ok(());
            }
            if meta.path.is_ident("skip") || meta.path.is_ident("skip_deserializing") {
                out.skip = true;
                return Ok(());
            }
            if meta.path.is_ident("flatten") {
                out.flatten = true;
                return Ok(());
            }
            if meta.path.is_ident("default") {
                if meta.input.peek(syn::Token![=]) {
                    // serde spells the factory as a string path: `default = "f"`.
                    let lit: LitStr = meta.value()?.parse()?;
                    let path: syn::Path = lit.parse()?;
                    out.default_value =
                        Some(DefaultValue::Expr(syn::parse_quote!(#path())));
                } else {
                    out.default_value = Some(DefaultValue::Trait);
                }
                return Ok(());
            }
            skip_meta_value(&meta)
        });
    }
    Ok(out)
}

/// Consume (and discard) the value of a serde key this derive does not model,
/// so parsing keeps going instead of failing the whole attribute.
fn skip_meta_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        let _content;
        syn::parenthesized!(_content in meta.input);
        let _: proc_macro2::TokenStream = _content.parse()?;
    }
    Ok(())
}

pub fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_inner(input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_inner(input: DeriveInput) -> syn::Result<TokenStream> {
    let krate = r2e_core_path();
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(f) => &f.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "Params can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "Params can only be derived for structs",
            ))
        }
    };

    // `#[serde(rename_all = "…")]` on the struct renames every field whose
    // key is derived from its Rust ident — exactly as serde would when the
    // same struct is deserialized through `Query<T>`.
    let rename_all = parse_serde_rename_all(&input.attrs)?;

    let mut parsed_fields = Vec::new();

    for field in fields {
        let ident = field.ident.clone().unwrap();
        let ty = field.ty.clone();
        let is_optional = is_option_type(&ty);

        let serde_attrs = parse_serde_field_attrs(&field.attrs)?;
        // The key this field is read under when nothing names it explicitly:
        // `#[serde(rename)]` wins over `rename_all`, which wins over the ident.
        let default_name = match (&serde_attrs.rename, rename_all) {
            (Some(renamed), _) => renamed.clone(),
            (None, Some(case)) => case.apply(&ident.to_string()),
            (None, None) => ident.to_string(),
        };

        let mut source = None;
        // `#[param(default …)]` wins over `#[serde(default …)]` when both are
        // present, since the r2e spelling is the more specific one.
        let mut default_value = serde_attrs.default_value.clone();
        let mut nested_mode = None;
        let mut has_r2e_attr = false;

        for attr in &field.attrs {
            if attr.path().is_ident("query") {
                has_r2e_attr = true;
                let custom_name = parse_name_attr(attr)?;
                let name = custom_name.unwrap_or_else(|| default_name.clone());
                source = Some(ParamSource::Query { name });
            } else if attr.path().is_ident("header") {
                has_r2e_attr = true;
                let header_name: LitStr = attr.parse_args()?;
                source = Some(ParamSource::Header {
                    name: header_name.value(),
                });
            } else if attr.path().is_ident("param") {
                has_r2e_attr = true;
                let options = parse_param_options(attr, &default_name)?;
                if let Some(param_source) = options.source {
                    source = Some(param_source);
                }
                if let Some(param_default) = options.default_value {
                    default_value = Some(param_default);
                }
            } else if attr.path().is_ident("params") {
                has_r2e_attr = true;
                nested_mode = Some(parse_nested_mode(attr, &ident)?);
            }
        }

        // Error if #[params] is combined with #[param(path)], #[query], or #[header]
        if nested_mode.is_some() && source.is_some() {
            return Err(syn::Error::new_spanned(
                &ident,
                "#[params] cannot be combined with #[param(path)], #[query], or #[header]",
            ));
        }

        // `#[serde(skip)]` / `#[serde(skip_deserializing)]`: not a parameter at
        // all. Filled with `Default::default()`, like serde does.
        if serde_attrs.skip {
            if has_r2e_attr {
                return Err(syn::Error::new_spanned(
                    &ident,
                    "#[serde(skip)] cannot be combined with #[query], #[header], #[param], or #[params] \
                     — a skipped field is never read from the request",
                ));
            }
            parsed_fields.push(ParsedField::Skipped(ident));
            continue;
        }

        // `#[serde(flatten)]` reads the nested struct's own keys from the same
        // request — the exact meaning of a bare `#[params]`.
        if serde_attrs.flatten && nested_mode.is_none() {
            if has_r2e_attr {
                return Err(syn::Error::new_spanned(
                    &ident,
                    "#[serde(flatten)] cannot be combined with #[query], #[header] or #[param] \
                     — a flattened field is a nested parameter group, not a single parameter. \
                     Use #[params(prefix = \"...\")] to give it a prefix instead",
                ));
            }
            nested_mode = Some(NestedMode::Flatten);
        }

        if let Some(mode) = nested_mode {
            parsed_fields.push(ParsedField::Nested(NestedParamsField { ident, ty, mode }));
        } else {
            // No source attribute at all → a query parameter under its
            // (possibly serde-renamed) name. This is what makes a struct
            // written for `Query<T>` compile as `Params` untouched.
            let source = source.unwrap_or(ParamSource::Query {
                name: default_name.clone(),
            });
            parsed_fields.push(ParsedField::Param(ParamField {
                ident,
                ty,
                source,
                is_optional,
                default_value,
            }));
        }
    }

    // Separate param fields and nested fields
    let param_fields: Vec<&ParamField> = parsed_fields
        .iter()
        .filter_map(|f| match f {
            ParsedField::Param(p) => Some(p),
            _ => None,
        })
        .collect();
    let nested_fields: Vec<&NestedParamsField> = parsed_fields
        .iter()
        .filter_map(|f| match f {
            ParsedField::Nested(n) => Some(n),
            _ => None,
        })
        .collect();

    let has_path_fields = param_fields
        .iter()
        .any(|f| matches!(f.source, ParamSource::Path { .. }));
    let has_query_fields = param_fields
        .iter()
        .any(|f| matches!(f.source, ParamSource::Query { .. }));
    // Nested fields may contain query fields, so always parse query if nested fields exist
    let needs_query = has_query_fields || !nested_fields.is_empty();

    // Generate extraction code for path params
    let path_extraction = if has_path_fields {
        quote! {
            let __raw_path = <#krate::http::extract::RawPathParams as #krate::http::extract::FromRequestParts<__R2eParamsState>>::from_request_parts(parts, _state)
                .await
                .map_err(|e| {
                    let err = #krate::web::params::ParamError {
                        message: format!("Failed to extract path parameters: {}", e),
                    };
                    #krate::http::response::IntoResponse::into_response(err)
                })?;
        }
    } else {
        quote! {}
    };

    // Generate extraction code for query params
    let query_extraction = if needs_query {
        quote! {
            let __query_pairs = #krate::web::params::parse_query_string(parts.uri.query());
        }
    } else {
        quote! {}
    };

    // Generate field construction for param fields (now prefix-aware for query)
    let field_constructions: Vec<TokenStream> = param_fields
        .iter()
        .map(|f| generate_field_construction(f, &krate))
        .collect();

    // Generate field construction for nested fields
    let nested_constructions: Vec<TokenStream> = nested_fields
        .iter()
        .map(|f| generate_nested_construction(f, &krate))
        .collect();

    // Skipped fields are not extracted — they are defaulted.
    let skipped_constructions: Vec<TokenStream> = parsed_fields
        .iter()
        .filter_map(|f| match f {
            ParsedField::Skipped(ident) => Some(quote! {
                let #ident = ::core::default::Default::default();
            }),
            _ => None,
        })
        .collect();

    // Collect all field idents (param, nested and skipped) in declaration order
    let all_field_idents: Vec<&Ident> = parsed_fields
        .iter()
        .map(|f| match f {
            ParsedField::Param(p) => &p.ident,
            ParsedField::Nested(n) => &n.ident,
            ParsedField::Skipped(ident) => ident,
        })
        .collect();

    // Generate metadata
    let own_param_info_items = generate_param_infos(&param_fields, &krate);
    let nested_metadata_items: Vec<TokenStream> = nested_fields
        .iter()
        .map(|f| generate_nested_metadata(f, &krate))
        .collect();

    let expanded = quote! {
        const _: () = {
            // Core impl: PrefixedExtract receives the prefix and threads it through
            impl<__R2eParamsState: Send + Sync> #krate::web::params::PrefixedExtract<__R2eParamsState> for #name #ty_generics {
                async fn extract_prefixed(
                    parts: &mut #krate::http::header::Parts,
                    _state: &__R2eParamsState,
                    __prefix: &str,
                ) -> Result<Self, #krate::http::response::Response> {
                    use #krate::http::response::IntoResponse as _;

                    #path_extraction
                    #query_extraction

                    #(#field_constructions)*
                    #(#nested_constructions)*
                    #(#skipped_constructions)*

                    Ok(Self {
                        #(#all_field_idents,)*
                    })
                }
            }

            // Thin wrapper: delegates to PrefixedExtract with empty prefix
            // Named bridge point (plan §5.3b): a `#[derive(Params)]` struct is
            // used as a route-method parameter, which the HTTP backend extracts.
            impl<__R2eParamsState: Send + Sync> #krate::http::extract::FromRequestParts<__R2eParamsState> for #name #ty_generics {
                type Rejection = #krate::http::response::Response;

                async fn from_request_parts(
                    parts: &mut #krate::http::header::Parts,
                    _state: &__R2eParamsState,
                ) -> Result<Self, Self::Rejection> {
                    <Self as #krate::web::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, "").await
                }
            }

            impl #impl_generics #krate::web::params::ParamsMetadata for #name #ty_generics #where_clause {
                fn param_infos() -> Vec<#krate::di::meta::ParamInfo> {
                    let mut __v = vec![#(#own_param_info_items),*];
                    #(#nested_metadata_items)*
                    __v
                }
            }
        };
    };

    Ok(expanded)
}

/// Generate the query key lookup expression, prefix-aware.
/// For query fields: uses `prefixed_key(__prefix, "name")` so nesting composes.
/// For path/header: prefix doesn't apply — they use the raw name.
fn generate_field_construction(field: &ParamField, krate: &TokenStream) -> TokenStream {
    let ident = &field.ident;
    let name_str = match &field.source {
        ParamSource::Path { name } => name.as_str(),
        ParamSource::Query { name } => name.as_str(),
        ParamSource::Header { name } => name.as_str(),
    };

    let missing_fallback = |error_msg: &str| -> TokenStream {
        match &field.default_value {
            Some(DefaultValue::Trait) => quote! { Default::default() },
            Some(DefaultValue::Expr(expr)) => quote! { (#expr).into() },
            None => {
                let msg = error_msg.to_string();
                quote! {
                    return Err(#krate::http::response::IntoResponse::into_response(
                        #krate::web::params::ParamError {
                            message: #msg.to_string(),
                        }
                    ))
                }
            }
        }
    };

    match &field.source {
        ParamSource::Path { .. } => {
            // Path params are never prefixed
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = match __raw_path.iter().find(|(k, _)| *k == #name_str) {
                        Some((_, v)) => {
                            match v.parse() {
                                Ok(val) => Some(val),
                                Err(_) => return Err(#krate::http::response::IntoResponse::into_response(
                                    #krate::web::params::ParamError {
                                        message: format!("Invalid path parameter '{}': parse error", #name_str),
                                    }
                                )),
                            }
                        }
                        None => None,
                    };
                }
            } else {
                let fallback = missing_fallback(&format!("Missing path parameter '{}'", name_str));
                quote! {
                    let #ident = match __raw_path.iter().find(|(k, _)| *k == #name_str) {
                        Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                            #krate::web::params::ParamError {
                                message: format!("Invalid path parameter '{}': parse error", #name_str),
                            }
                        ))?,
                        None => #fallback,
                    };
                }
            }
        }
        ParamSource::Query { .. } => {
            // Query params are prefix-aware
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = {
                        let __key = #krate::web::params::prefixed_key(__prefix, #name_str);
                        match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                            Some((_, v)) => Some(v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::web::params::ParamError {
                                    message: format!("Invalid query parameter '{}': parse error", __key),
                                }
                            ))?),
                            None => None,
                        }
                    };
                }
            } else {
                match &field.default_value {
                    Some(DefaultValue::Trait) => {
                        quote! {
                            let #ident = {
                                let __key = #krate::web::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::web::params::ParamError {
                                            message: format!("Invalid query parameter '{}': parse error", __key),
                                        }
                                    ))?,
                                    None => Default::default(),
                                }
                            };
                        }
                    }
                    Some(DefaultValue::Expr(expr)) => {
                        quote! {
                            let #ident = {
                                let __key = #krate::web::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::web::params::ParamError {
                                            message: format!("Invalid query parameter '{}': parse error", __key),
                                        }
                                    ))?,
                                    None => (#expr).into(),
                                }
                            };
                        }
                    }
                    None => {
                        quote! {
                            let #ident = {
                                let __key = #krate::web::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::web::params::ParamError {
                                            message: format!("Invalid query parameter '{}': parse error", __key),
                                        }
                                    ))?,
                                    None => return Err(#krate::http::response::IntoResponse::into_response(
                                        #krate::web::params::ParamError {
                                            message: format!("Missing query parameter '{}'", __key),
                                        }
                                    )),
                                }
                            };
                        }
                    }
                }
            }
        }
        ParamSource::Header { .. } => {
            // Header params are never prefixed
            if field.is_optional {
                let inner_ty = unwrap_option_type(&field.ty).unwrap();
                quote! {
                    let #ident: Option<#inner_ty> = match parts.headers.get(#name_str) {
                        Some(v) => {
                            let s = v.to_str().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::web::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            Some(s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::web::params::ParamError {
                                    message: format!("Invalid header '{}': parse error", #name_str),
                                }
                            ))?)
                        }
                        None => None,
                    };
                }
            } else {
                let fallback = missing_fallback(&format!("Missing required header '{}'", name_str));
                quote! {
                    let #ident = match parts.headers.get(#name_str) {
                        Some(v) => {
                            let s = v.to_str().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::web::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::web::params::ParamError {
                                    message: format!("Invalid header '{}': parse error", #name_str),
                                }
                            ))?
                        }
                        None => #fallback,
                    };
                }
            }
        }
    }
}

/// Generate extraction code for a nested `#[params]` field.
fn generate_nested_construction(field: &NestedParamsField, krate: &TokenStream) -> TokenStream {
    let ident = &field.ident;
    let ty = &field.ty;

    match &field.mode {
        NestedMode::Flatten => {
            // Flatten: pass through the parent prefix unchanged
            quote! {
                let #ident = <#ty as #krate::web::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, __prefix).await?;
            }
        }
        NestedMode::Prefix(prefix_str) => {
            // Prefix: compose parent prefix with this field's prefix
            quote! {
                let #ident = {
                    let __composed = if __prefix.is_empty() {
                        #prefix_str.to_string()
                    } else {
                        format!("{}.{}", __prefix, #prefix_str)
                    };
                    <#ty as #krate::web::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, &__composed).await?
                };
            }
        }
    }
}

/// Generate metadata extension code for a nested `#[params]` field.
fn generate_nested_metadata(field: &NestedParamsField, krate: &TokenStream) -> TokenStream {
    let ty = &field.ty;

    match &field.mode {
        NestedMode::Flatten => {
            // Flatten: merge all nested param infos unchanged
            quote! {
                __v.extend(<#ty as #krate::web::params::ParamsMetadata>::param_infos());
            }
        }
        NestedMode::Prefix(prefix_str) => {
            // Prefix: prefix query param names at metadata level
            quote! {
                __v.extend(<#ty as #krate::web::params::ParamsMetadata>::param_infos().into_iter().map(|mut p| {
                    if matches!(p.location, #krate::di::meta::ParamLocation::Query) {
                        p.name = format!("{}.{}", #prefix_str, p.name);
                    }
                    p
                }));
            }
        }
    }
}

/// Parse `#[params]`, `#[params(prefix)]`, or `#[params(prefix = "custom")]`
fn parse_nested_mode(attr: &syn::Attribute, field_ident: &Ident) -> syn::Result<NestedMode> {
    match &attr.meta {
        syn::Meta::Path(_) => {
            // #[params] — flatten
            Ok(NestedMode::Flatten)
        }
        syn::Meta::List(_) => {
            let mut mode = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("prefix") {
                    if meta.input.peek(syn::Token![=]) {
                        // #[params(prefix = "custom")]
                        let value = meta.value()?;
                        let lit: LitStr = value.parse()?;
                        mode = Some(NestedMode::Prefix(lit.value()));
                    } else {
                        // #[params(prefix)] — use field name as prefix
                        mode = Some(NestedMode::Prefix(field_ident.to_string()));
                    }
                    Ok(())
                } else {
                    Err(meta.error("expected `prefix` or `prefix = \"...\"`"))
                }
            })?;
            mode.ok_or_else(|| {
                syn::Error::new_spanned(
                    attr,
                    "expected #[params], #[params(prefix)], or #[params(prefix = \"...\")]",
                )
            })
        }
        _ => Err(syn::Error::new_spanned(
            attr,
            "expected #[params], #[params(prefix)], or #[params(prefix = \"...\")]",
        )),
    }
}

fn parse_param_options(attr: &syn::Attribute, default_name: &str) -> syn::Result<ParamOptions> {
    let mut is_path = false;
    let mut path_name = None;
    let mut default_value = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("path") {
            is_path = true;
            Ok(())
        } else if meta.path.is_ident("name") {
            let value = meta.value()?;
            let lit: LitStr = value.parse()?;
            path_name = Some(lit.value());
            Ok(())
        } else if meta.path.is_ident("default") {
            if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let expr: Expr = value.parse()?;
                default_value = Some(DefaultValue::Expr(expr));
            } else {
                default_value = Some(DefaultValue::Trait);
            }
            Ok(())
        } else {
            Err(meta.error("expected `path`, `name = \"...\"`, `default`, or `default = <expr>`"))
        }
    })?;

    if path_name.is_some() && !is_path {
        return Err(syn::Error::new_spanned(
            attr,
            "`name` requires `path`: use #[param(path, name = \"...\")]",
        ));
    }

    if !is_path && default_value.is_none() {
        return Err(syn::Error::new_spanned(
            attr,
            "expected #[param(path)], #[param(default)], or a combination of both",
        ));
    }

    let source = is_path.then(|| ParamSource::Path {
        name: path_name.unwrap_or_else(|| default_name.to_string()),
    });

    Ok(ParamOptions {
        source,
        default_value,
    })
}

fn parse_name_attr(attr: &syn::Attribute) -> syn::Result<Option<String>> {
    match attr.meta {
        syn::Meta::Path(_) => Ok(None),
        syn::Meta::List(_) => {
            let mut name = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `name = \"...\"`"))
                }
            })?;
            Ok(name)
        }
        _ => Ok(None),
    }
}

use crate::util::type_utils::{is_option_type, unwrap_option_type};

/// Map a Rust type to an OpenAPI type string.
fn rust_type_to_openapi_str(ty: &Type) -> &'static str {
    let inner = unwrap_option_type(ty).unwrap_or(ty);
    if let Type::Path(type_path) = inner {
        if let Some(segment) = type_path.path.segments.last() {
            return match segment.ident.to_string().as_str() {
                "String" | "str" => "string",
                "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" => {
                    "integer"
                }
                "f32" | "f64" => "number",
                "bool" => "boolean",
                _ => "string",
            };
        }
    }
    "string"
}

/// Generate `ParamInfo` literal tokens for each parsed field.
fn generate_param_infos(
    fields: &[&ParamField],
    krate: &proc_macro2::TokenStream,
) -> Vec<proc_macro2::TokenStream> {
    fields
        .iter()
        .map(|f| {
            let (param_name, location) = match &f.source {
                ParamSource::Path { name } => (
                    name.clone(),
                    quote! { #krate::di::meta::ParamLocation::Path },
                ),
                ParamSource::Query { name } => (
                    name.clone(),
                    quote! { #krate::di::meta::ParamLocation::Query },
                ),
                ParamSource::Header { name } => (
                    name.clone(),
                    quote! { #krate::di::meta::ParamLocation::Header },
                ),
            };
            let param_type = rust_type_to_openapi_str(&f.ty);
            let required = !f.is_optional && f.default_value.is_none();

            quote! {
                #krate::di::meta::ParamInfo {
                    name: #param_name.to_string(),
                    location: #location,
                    param_type: #param_type.to_string(),
                    required: #required,
                }
            }
        })
        .collect()
}
