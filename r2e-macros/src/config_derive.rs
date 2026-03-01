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
    key: Option<String>,
}

/// Extract `#[config(default = <expr>, key = "...")]` from a field's attributes.
fn extract_field_config(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut result = FieldConfig {
        default: None,
        key: None,
    };
    for attr in attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("default") {
                    let value = meta.value()?;
                    let lit: syn::Expr = value.parse()?;
                    let lit_str = quote!(#lit).to_string();
                    result.default = Some((quote!(#lit), lit_str));
                    Ok(())
                } else if meta.path.is_ident("key") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.key = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `default` or `key` in #[config(...)]"))
                }
            })?;
        }
    }
    Ok(result)
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
            Some((expr, s)) => (Some(expr), Some(s)),
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
        });
    }

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
            let required = !f.is_option && f.default_expr.is_none();
            let default_val = match &f.default_str {
                Some(s) => quote! { Some(#s.to_string()) },
                None => quote! { None },
            };
            let desc = match &f.doc {
                Some(d) => quote! { Some(#d.to_string()) },
                None => quote! { None },
            };
            quote! {
                #krate::config::typed::PropertyMeta {
                    key: #key.to_string(),
                    full_key: #full_key.to_string(),
                    type_name: #type_name_simple,
                    required: #required,
                    default_value: #default_val,
                    description: #desc,
                }
            }
        })
        .collect();

    // Generate from_config field initializers
    let field_inits: Vec<TokenStream2> = field_infos
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key_str = match &f.custom_key {
                Some(k) => k.clone(),
                None => f.name.to_string(),
            };
            let full_key = format!("{}.{}", prefix, key_str);

            if f.is_option {
                // Option<T> field: return Ok(None) if not found
                let inner_ty = option_inner_type(&f.ty).unwrap();
                if let Some(default_expr) = &f.default_expr {
                    quote! {
                        #field_name: match config.get::<#inner_ty>(#full_key) {
                            Ok(v) => Some(v),
                            Err(#krate::config::ConfigError::NotFound(_)) => Some(#default_expr.into()),
                            Err(e) => return Err(e),
                        }
                    }
                } else {
                    quote! {
                        #field_name: match config.get::<#inner_ty>(#full_key) {
                            Ok(v) => Some(v),
                            Err(#krate::config::ConfigError::NotFound(_)) => None,
                            Err(e) => return Err(e),
                        }
                    }
                }
            } else if let Some(default_expr) = &f.default_expr {
                // Non-option with default
                let ty = &f.ty;
                quote! {
                    #field_name: config.get_or::<#ty>(#full_key, #default_expr.into())
                }
            } else {
                // Required field
                let ty = &f.ty;
                quote! {
                    #field_name: config.get::<#ty>(#full_key)?
                }
            }
        })
        .collect();

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

            fn from_config(config: &#krate::config::R2eConfig) -> Result<Self, #krate::config::ConfigError> {
                Ok(Self {
                    #(#field_inits,)*
                })
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
