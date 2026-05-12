use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::type_utils::{parse_config_field, parse_config_section_prefix, parse_inject_name};

pub enum FieldKind {
    Inject,
    InjectNamed { name: String },
    Config {
        key: String,
        env_hint: String,
        ty_name: String,
    },
    ConfigSection {
        prefix: String,
    },
    Default,
}

pub struct ClassifiedField<'a> {
    pub name: &'a syn::Ident,
    pub ty: &'a syn::Type,
    pub kind: FieldKind,
}

pub struct ClassifyOpts {
    pub allow_named_inject: bool,
    pub allow_default: bool,
    pub context_label: &'static str,
}

pub fn classify_fields<'a>(
    fields: impl Iterator<Item = &'a syn::Field>,
    opts: &ClassifyOpts,
) -> syn::Result<Vec<ClassifiedField<'a>>> {
    let mut result = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("config_section"));
        let is_default = field.attrs.iter().any(|a| a.path().is_ident("default"));

        if is_inject {
            let named = if opts.allow_named_inject {
                parse_inject_name(&field.attrs)?
            } else {
                None
            };
            let kind = match named {
                Some(name) => FieldKind::InjectNamed { name },
                None => FieldKind::Inject,
            };
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind,
            });
        } else if let Some(attr) = config_section_attr {
            let prefix = parse_config_section_prefix(attr)?;
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::ConfigSection { prefix },
            });
        } else if let Some(attr) = config_attr {
            let (key, env_hint, ty_name) = parse_config_field(attr, field_type)?;
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::Config {
                    key,
                    env_hint,
                    ty_name,
                },
            });
        } else if is_default && opts.allow_default {
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::Default,
            });
        } else {
            let mut hints = vec!["#[inject]                           — clone from app state"];
            if opts.allow_named_inject {
                hints.push("#[inject(name = \"...\")]             — named injection via newtype");
            }
            hints.push("#[config(\"app.key\")]                — resolve from R2eConfig");
            hints.push("#[config_section(prefix = \"app\")]   — resolve a typed config section");
            if opts.allow_default {
                hints.push("#[default]                          — use `Default::default()`");
            }
            let msg = format!(
                "{} field must be annotated with one of:\n{}",
                opts.context_label,
                hints
                    .iter()
                    .map(|h| format!("\n  {h}"))
                    .collect::<String>()
            );
            return Err(syn::Error::new_spanned(field_name, msg));
        }
    }

    Ok(result)
}

pub fn config_init_panic(
    field_name: &syn::Ident,
    key: &str,
    env_hint: &str,
    owner_name: &str,
) -> TokenStream2 {
    quote! {
        #field_name: __cfg.get(#key).unwrap_or_else(|e| panic!(
            "Configuration error in `{}`: key '{}' — {}. \
             Add it to application.yaml / application-{{profile}}.yaml, \
             or set env var `{}`.",
            #owner_name, #key, e, #env_hint
        ))
    }
}

pub fn config_section_init_panic(
    field_name: &syn::Ident,
    field_type: &syn::Type,
    prefix: &str,
    owner_name: &str,
    krate: &TokenStream2,
) -> TokenStream2 {
    quote! {
        #field_name: <#field_type as #krate::ConfigProperties>::from_config(&__cfg, Some(#prefix))
            .unwrap_or_else(|e| panic!(
                "Configuration error in `{}`: failed to load section '{}' — {}",
                #owner_name, #prefix, e,
            ))
    }
}
