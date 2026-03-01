use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Lit, Meta};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Extract the `#[config(prefix = "...")]` attribute from the struct.
fn extract_prefix(input: &DeriveInput) -> syn::Result<String> {
    for attr in &input.attrs {
        if attr.path().is_ident("config") {
            let mut prefix = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("prefix") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    prefix = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `prefix` in #[config(prefix = \"...\")]"))
                }
            })?;
            if let Some(p) = prefix {
                return Ok(p);
            }
        }
    }
    Err(syn::Error::new_spanned(
        &input.ident,
        "#[derive(ConfigProperties)] requires #[config(prefix = \"...\")]\n\
         \n  example:\n  #[derive(ConfigProperties)]\n  #[config(prefix = \"app.database\")]\n  pub struct DatabaseConfig { ... }",
    ))
}

/// Parsed information about a single field in a ConfigProperties struct.
struct FieldInfo {
    name: syn::Ident,
    ty: syn::Type,
    /// The config default value expression, if any.
    default_expr: Option<TokenStream2>,
    /// The config default value as string for metadata.
    default_str: Option<String>,
    /// Custom config key override from `#[config(key = "...")]`.
    custom_key: Option<String>,
    /// Whether the field is Option<T>.
    is_option: bool,
    /// Doc comment text.
    doc: Option<String>,
    /// Whether this is a `#[config(section)]` — nested ConfigProperties.
    is_section: bool,
    /// Explicit env var from `#[config(env = "...")]`.
    env_var: Option<String>,
    /// Whether the field has any `#[validate(...)]` attributes (from garde).
    has_validate: bool,
}

/// Check if a type is `Option<T>`.
fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(syn::TypePath { path, .. }) = ty {
        if let Some(seg) = path.segments.last() {
            return seg.ident == "Option";
        }
    }
    false
}

/// Extract the inner type from `Option<T>`.
fn option_inner_type(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(syn::TypePath { path, .. }) = ty {
        if let Some(seg) = path.segments.last() {
            if seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Extract doc comments from a field's attributes.
fn extract_doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if attr.path().is_ident("doc") {
                if let Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: Lit::Str(s), ..
                    }) = &nv.value
                    {
                        return Some(s.value().trim().to_string());
                    }
                }
            }
            None
        })
        .collect();
    if docs.is_empty() {
        None
    } else {
        Some(docs.join(" "))
    }
}

/// Parsed field-level `#[config(...)]` attributes.
struct FieldConfig {
    default: Option<(TokenStream2, String)>,
    /// True if the default expression is a string literal (needs `.into()` for String conversion).
    default_is_str_lit: bool,
    key: Option<String>,
    section: bool,
    env: Option<String>,
}

/// Extract `#[config(default = <expr>, key = "...", section, env = "...")]` from a field's attributes.
fn extract_field_config(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut result = FieldConfig {
        default: None,
        default_is_str_lit: false,
        key: None,
        section: false,
        env: None,
    };
    for attr in attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("default") {
                    let value = meta.value()?;
                    let lit: syn::Expr = value.parse()?;
                    let lit_str = quote!(#lit).to_string();
                    // Detect string literals — they need .into() for &str → String
                    if matches!(&lit, syn::Expr::Lit(syn::ExprLit { lit: Lit::Str(_), .. })) {
                        result.default_is_str_lit = true;
                    }
                    result.default = Some((quote!(#lit), lit_str));
                    Ok(())
                } else if meta.path.is_ident("key") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.key = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("section") {
                    result.section = true;
                    Ok(())
                } else if meta.path.is_ident("env") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.env = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `default`, `key`, `section`, or `env` in #[config(...)]"))
                }
            })?;
        }
    }
    Ok(result)
}

/// Check if a field has any `#[validate(...)]` attributes (from garde).
fn has_validate_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("validate"))
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let prefix = extract_prefix(input)?;
    let krate = r2e_core_path();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(ConfigProperties)] only works on structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(ConfigProperties)] only works on structs",
            ))
        }
    };

    // Parse all fields
    let mut field_infos = Vec::new();
    for field in fields {
        let field_name = field.ident.clone().unwrap();
        let field_type = field.ty.clone();
        let is_option = is_option_type(&field_type);
        let doc = extract_doc_comment(&field.attrs);
        let field_config = extract_field_config(&field.attrs)?;
        let (default_expr, default_str) = match field_config.default {
            Some((expr, s)) => {
                // String literals need .into() for &str → String conversion.
                // Other types (int, float, bool) work with direct assignment
                // via type inference from the target type annotation.
                let expr = if field_config.default_is_str_lit {
                    quote!(#expr.into())
                } else {
                    expr
                };
                (Some(expr), Some(s))
            }
            None => (None, None),
        };

        field_infos.push(FieldInfo {
            name: field_name,
            ty: field_type,
            default_expr,
            default_str,
            custom_key: field_config.key,
            is_option,
            doc,
            is_section: field_config.section,
            env_var: field_config.env,
            has_validate: has_validate_attr(&field.attrs),
        });
    }

    // Detect if any field has garde validation → we'll call validate() after construction
    let any_has_validate = field_infos.iter().any(|f| f.has_validate);

    // Generate metadata entries
    let metadata_entries: Vec<TokenStream2> = field_infos
        .iter()
        .map(|f| {
            let key = match &f.custom_key {
                Some(k) => k.clone(),
                None => f.name.to_string(),
            };
            let full_key = format!("{}.{}", prefix, key);
            let type_name_simple = type_name_str(&f.ty);
            let required = !f.is_option && f.default_expr.is_none() && !f.is_section;
            let default_val = match &f.default_str {
                Some(s) => quote! { Some(#s.to_string()) },
                None => quote! { None },
            };
            let desc = match &f.doc {
                Some(d) => quote! { Some(#d.to_string()) },
                None => quote! { None },
            };
            let env_var_tok = match &f.env_var {
                Some(e) => quote! { Some(#e.to_string()) },
                None => quote! { None },
            };
            let is_section = f.is_section;
            quote! {
                #krate::config::typed::PropertyMeta {
                    key: #key.to_string(),
                    full_key: #full_key.to_string(),
                    type_name: #type_name_simple,
                    required: #required,
                    default_value: #default_val,
                    description: #desc,
                    env_var: #env_var_tok,
                    is_section: #is_section,
                }
            }
        })
        .collect();

    // Generate from_config_prefixed field initializers
    let field_inits: Vec<TokenStream2> = field_infos
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key_str = match &f.custom_key {
                Some(k) => k.clone(),
                None => f.name.to_string(),
            };

            // Section fields delegate to ConfigProperties::from_config_prefixed
            if f.is_section {
                let section_ty = if f.is_option {
                    option_inner_type(&f.ty).unwrap()
                } else {
                    &f.ty
                };
                let section_init = quote! {
                    <#section_ty as #krate::config::ConfigProperties>::from_config_prefixed(
                        __config,
                        &format!("{}.{}", __prefix, #key_str),
                    )
                };
                if f.is_option {
                    return quote! {
                        #field_name: match #section_init {
                            Ok(v) => Some(v),
                            Err(#krate::config::ConfigError::NotFound(_)) => None,
                            Err(e) => return Err(e),
                        }
                    };
                }
                return quote! {
                    #field_name: #section_init?
                };
            }

            // Build the key expression using runtime prefix
            let key_expr = quote! { &format!("{}.{}", __prefix, #key_str) };

            // Helper: generate env var fallback block that converts the
            // env string to the target type via FromConfigValue.
            let env_try = |_target_ty: &syn::Type, env_name: &str| {
                quote! {
                    match std::env::var(#env_name) {
                        Ok(__env_val) => {
                            let __cv = #krate::config::ConfigValue::String(__env_val);
                            match #krate::config::FromConfigValue::from_config_value(&__cv, __key) {
                                Ok(__v) => __v,
                                Err(__e) => return Err(__e),
                            }
                        }
                        Err(_) => return Err(#krate::config::ConfigError::NotFound(__key.to_string())),
                    }
                }
            };

            if f.is_option {
                let inner_ty = option_inner_type(&f.ty).unwrap();

                // Option<T> — env + default
                if let (Some(env_name), Some(default_expr)) = (&f.env_var, &f.default_expr) {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            Some(#krate::config::FromConfigValue::from_config_value(&__cv, __key)?)
                                        }
                                        Err(_) => { let __d: #inner_ty = #default_expr; Some(__d) }
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — env only
                else if let Some(env_name) = &f.env_var {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            Some(#krate::config::FromConfigValue::from_config_value(&__cv, __key)?)
                                        }
                                        Err(_) => None,
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — default only
                else if let Some(default_expr) = &f.default_expr {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => { let __d: #inner_ty = (#default_expr).into(); Some(__d) }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — plain
                else {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => None,
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            } else if let Some(default_expr) = &f.default_expr {
                // Required with default — env + default
                let ty = &f.ty;
                if let Some(env_name) = &f.env_var {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            #krate::config::FromConfigValue::from_config_value(&__cv, __key)?
                                        }
                                        Err(_) => { let __d: #ty = #default_expr; __d }
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                } else {
                    // Required with default — no env
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => { let __d: #ty = #default_expr; __d }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            } else {
                // Required — no default
                let ty = &f.ty;
                if let Some(env_name) = &f.env_var {
                    let env_block = env_try(ty, env_name);
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => { #env_block }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                } else {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            __config.get::<#ty>(__key)?
                        }
                    }
                }
            }
        })
        .collect();

    // Generate optional garde validation call
    let validation_call = if any_has_validate {
        quote! {
            {
                use garde::Validate as _;
                let __ctx = <#name as garde::Validate>::Context::default();
                __instance.validate(&__ctx).map_err(|__report| {
                    let __details = __report.iter()
                        .map(|(path, error)| #krate::config::ConfigValidationDetail {
                            key: format!("{}.{}", __prefix, path),
                            message: error.message().to_string(),
                        })
                        .collect();
                    #krate::config::ConfigError::Validation(__details)
                })?;
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #krate::config::typed::ConfigProperties for #name {
            fn prefix() -> &'static str {
                #prefix
            }

            fn properties_metadata() -> Vec<#krate::config::typed::PropertyMeta> {
                vec![
                    #(#metadata_entries,)*
                ]
            }

            fn from_config_prefixed(
                __config: &#krate::config::R2eConfig,
                __prefix: &str,
            ) -> Result<Self, #krate::config::ConfigError> {
                let __instance = Self {
                    #(#field_inits,)*
                };
                #validation_call
                Ok(__instance)
            }
        }
    })
}

/// Get a simple type name string for metadata.
fn type_name_str(ty: &syn::Type) -> String {
    if let syn::Type::Path(syn::TypePath { path, .. }) = ty {
        if let Some(seg) = path.segments.last() {
            let name = seg.ident.to_string();
            if name == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return format!("Option<{}>", type_name_str(inner));
                    }
                }
            }
            if name == "Vec" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return format!("Vec<{}>", type_name_str(inner));
                    }
                }
            }
            return name;
        }
    }
    quote!(#ty).to_string()
}
