use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Ident, LitStr, Type};

use crate::crate_path::r2e_core_path;

enum ParamSource {
    Path { name: String },
    Query { name: String },
    Header { name: String },
}

enum DefaultValue {
    Trait,           // #[param(default)] → Default::default()
    Expr(Expr),      // #[param(default = 42)] → 42
}

struct ParamField {
    ident: Ident,
    ty: Type,
    source: ParamSource,
    is_optional: bool,
    default_value: Option<DefaultValue>,
}

enum NestedMode {
    Flatten,                    // #[params] — pass through parent prefix
    Prefix(String),             // #[params(prefix)] or #[params(prefix = "custom")]
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

    let mut parsed_fields = Vec::new();

    for field in fields {
        let ident = field.ident.clone().unwrap();
        let ty = field.ty.clone();
        let is_optional = is_option_type(&ty);

        let mut source = None;
        let mut default_value = None;
        let mut nested_mode = None;

        for attr in &field.attrs {
            if attr.path().is_ident("path") {
                let custom_name = parse_name_attr(attr)?;
                let name = custom_name.unwrap_or_else(|| ident.to_string());
                source = Some(ParamSource::Path { name });
            } else if attr.path().is_ident("query") {
                let custom_name = parse_name_attr(attr)?;
                let name = custom_name.unwrap_or_else(|| ident.to_string());
                source = Some(ParamSource::Query { name });
            } else if attr.path().is_ident("header") {
                let header_name: LitStr = attr.parse_args()?;
                source = Some(ParamSource::Header {
                    name: header_name.value(),
                });
            } else if attr.path().is_ident("param") {
                default_value = Some(parse_param_default(attr)?);
            } else if attr.path().is_ident("params") {
                nested_mode = Some(parse_nested_mode(attr, &ident)?);
            }
        }

        // Error if #[params] is combined with #[path], #[query], or #[header]
        if nested_mode.is_some() && source.is_some() {
            return Err(syn::Error::new_spanned(
                &ident,
                "#[params] cannot be combined with #[path], #[query], or #[header]",
            ));
        }

        if let Some(mode) = nested_mode {
            parsed_fields.push(ParsedField::Nested(NestedParamsField { ident, ty, mode }));
        } else if let Some(source) = source {
            parsed_fields.push(ParsedField::Param(ParamField {
                ident,
                ty,
                source,
                is_optional,
                default_value,
            }));
        }
        // Fields without any recognized attribute are ignored
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
                    let err = #krate::params::ParamError {
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
            let __query_pairs = #krate::params::parse_query_string(parts.uri.query());
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

    // Collect all field idents (both param and nested) in the order they appear
    let all_field_idents: Vec<&Ident> = parsed_fields
        .iter()
        .map(|f| match f {
            ParsedField::Param(p) => &p.ident,
            ParsedField::Nested(n) => &n.ident,
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
            impl<__R2eParamsState: Send + Sync> #krate::params::PrefixedExtract<__R2eParamsState> for #name #ty_generics {
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

                    Ok(Self {
                        #(#all_field_idents,)*
                    })
                }
            }

            // Thin wrapper: delegates to PrefixedExtract with empty prefix
            impl<__R2eParamsState: Send + Sync> #krate::http::extract::FromRequestParts<__R2eParamsState> for #name #ty_generics {
                type Rejection = #krate::http::response::Response;

                async fn from_request_parts(
                    parts: &mut #krate::http::header::Parts,
                    _state: &__R2eParamsState,
                ) -> Result<Self, Self::Rejection> {
                    <Self as #krate::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, "").await
                }
            }

            impl #impl_generics #krate::params::ParamsMetadata for #name #ty_generics #where_clause {
                fn param_infos() -> Vec<#krate::meta::ParamInfo> {
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
                        #krate::params::ParamError {
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
                    let #ident: Option<#inner_ty> = match __raw_path.iter().find(|(k, _)| k.as_str() == #name_str) {
                        Some((_, v)) => {
                            match v.parse() {
                                Ok(val) => Some(val),
                                Err(_) => return Err(#krate::http::response::IntoResponse::into_response(
                                    #krate::params::ParamError {
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
                    let #ident = match __raw_path.iter().find(|(k, _)| k.as_str() == #name_str) {
                        Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                            #krate::params::ParamError {
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
                        let __key = #krate::params::prefixed_key(__prefix, #name_str);
                        match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                            Some((_, v)) => Some(v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
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
                                let __key = #krate::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::params::ParamError {
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
                                let __key = #krate::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::params::ParamError {
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
                                let __key = #krate::params::prefixed_key(__prefix, #name_str);
                                match __query_pairs.iter().find(|(k, _)| k.as_str() == __key.as_ref()) {
                                    Some((_, v)) => v.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                        #krate::params::ParamError {
                                            message: format!("Invalid query parameter '{}': parse error", __key),
                                        }
                                    ))?,
                                    None => return Err(#krate::http::response::IntoResponse::into_response(
                                        #krate::params::ParamError {
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
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            Some(s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
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
                                #krate::params::ParamError {
                                    message: format!("Invalid header '{}': not valid UTF-8", #name_str),
                                }
                            ))?;
                            s.parse().map_err(|_| #krate::http::response::IntoResponse::into_response(
                                #krate::params::ParamError {
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
                let #ident = <#ty as #krate::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, __prefix).await?;
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
                    <#ty as #krate::params::PrefixedExtract<__R2eParamsState>>::extract_prefixed(parts, _state, &__composed).await?
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
                __v.extend(<#ty as #krate::params::ParamsMetadata>::param_infos());
            }
        }
        NestedMode::Prefix(prefix_str) => {
            // Prefix: prefix query param names at metadata level
            quote! {
                __v.extend(<#ty as #krate::params::ParamsMetadata>::param_infos().into_iter().map(|mut p| {
                    if matches!(p.location, #krate::meta::ParamLocation::Query) {
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
                syn::Error::new_spanned(attr, "expected #[params], #[params(prefix)], or #[params(prefix = \"...\")]")
            })
        }
        _ => Err(syn::Error::new_spanned(
            attr,
            "expected #[params], #[params(prefix)], or #[params(prefix = \"...\")]",
        )),
    }
}

fn parse_param_default(attr: &syn::Attribute) -> syn::Result<DefaultValue> {
    let mut result = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("default") {
            if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let expr: Expr = value.parse()?;
                result = Some(DefaultValue::Expr(expr));
            } else {
                result = Some(DefaultValue::Trait);
            }
            Ok(())
        } else {
            Err(meta.error("expected `default` or `default = <expr>`"))
        }
    })?;
    result.ok_or_else(|| syn::Error::new_spanned(attr, "expected #[param(default)] or #[param(default = <expr>)]"))
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

fn is_option_type(ty: &Type) -> bool {
    unwrap_option_type(ty).is_some()
}

fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Map a Rust type to an OpenAPI type string.
fn rust_type_to_openapi_str(ty: &Type) -> &'static str {
    let inner = unwrap_option_type(ty).unwrap_or(ty);
    if let Type::Path(type_path) = inner {
        if let Some(segment) = type_path.path.segments.last() {
            return match segment.ident.to_string().as_str() {
                "String" | "str" => "string",
                "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64"
                | "isize" => "integer",
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
                ParamSource::Path { name } => {
                    (name.clone(), quote! { #krate::meta::ParamLocation::Path })
                }
                ParamSource::Query { name } => {
                    (name.clone(), quote! { #krate::meta::ParamLocation::Query })
                }
                ParamSource::Header { name } => {
                    (name.clone(), quote! { #krate::meta::ParamLocation::Header })
                }
            };
            let param_type = rust_type_to_openapi_str(&f.ty);
            let required = !f.is_optional && f.default_value.is_none();

            quote! {
                #krate::meta::ParamInfo {
                    name: #param_name.to_string(),
                    location: #location,
                    param_type: #param_type.to_string(),
                    required: #required,
                }
            }
        })
        .collect()
}
