use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::hash_tokens::hash_token_stream;
use crate::type_list_gen::build_tcons_type;
use crate::type_utils::{parse_config_field, parse_config_section_prefix, parse_inject_name, named_bean_newtype_ident};

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let name_str = name.to_string();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(Bean)] only works on structs with named fields:\n\
                     \n  #[derive(Bean, Clone)]\n  struct MyService {\n      #[inject] dep: OtherService,\n  }",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(Bean)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    let krate = r2e_core_path();
    // Note: `Option<T>` fields under `#[inject]` are treated as hard
    // dependencies on the whole `Option<T>` type — a producer elsewhere in
    // the graph must register `Option<T>` for this bean to resolve.
    let mut dep_type_ids = Vec::new();
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut field_inits = Vec::new();
    let mut config_key_entries = Vec::new();
    let mut has_config = false;

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let inject_name = parse_inject_name(&field.attrs)?;
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr = field.attrs.iter().find(|a| a.path().is_ident("config_section"));
        let is_default = field.attrs.iter().any(|a| a.path().is_ident("default"));

        if is_inject {
            if let Some(name) = inject_name {
                // Named injection: resolve via generated newtype, unwrap with .0
                let newtype_ident = named_bean_newtype_ident(&name, field_type);
                dep_type_ids.push(quote! { (std::any::TypeId::of::<#newtype_ident>(), std::any::type_name::<#newtype_ident>()) });
                dep_types.push(quote! { #newtype_ident });
                field_inits.push(quote! { #field_name: ctx.get::<#newtype_ident>().0 });
            } else {
                // Regular inject — `Option<T>` fields land here too, keyed
                // under the whole `Option<T>` type.
                dep_type_ids.push(quote! { (std::any::TypeId::of::<#field_type>(), std::any::type_name::<#field_type>()) });
                dep_types.push(quote! { #field_type });
                field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
            }
        } else if let Some(attr) = config_section_attr {
            let prefix_str = parse_config_section_prefix(attr)?;
            field_inits.push(quote! {
                #field_name: #krate::config::ConfigProperties::from_config(&__r2e_config, Some(#prefix_str)).unwrap_or_else(|e| {
                    panic!(
                        "Configuration error in bean `{}`: config section '{}' — {}",
                        #name_str, #prefix_str, e
                    )
                })
            });
            has_config = true;
        } else if let Some(attr) = config_attr {
            let (key_str, env_hint, ty_name_str) = parse_config_field(attr, field_type)?;
            config_key_entries.push(quote! { (#key_str, #ty_name_str) });
            field_inits.push(quote! {
                #field_name: __r2e_config.get::<#field_type>(#key_str).unwrap_or_else(|_| {
                    panic!(
                        "Configuration error in bean `{}`: key '{}' — Config key not found. \
                         Add it to application.yaml or set env var `{}`.",
                        #name_str, #key_str, #env_hint
                    )
                })
            });
            has_config = true;
        } else if is_default {
            // Explicit opt-in: #[default] → Default::default()
            field_inits.push(quote! { #field_name: ::core::default::Default::default() });
        } else {
            return Err(syn::Error::new_spanned(
                field_name,
                "bean field must be annotated with one of:\n\
                 \n  #[inject]                           — resolve from the bean graph\n\
                 \n  #[inject(name = \"...\")]             — named injection via newtype\n\
                 \n  #[config(\"app.key\")]                — resolve from R2eConfig\n\
                 \n  #[config_section(prefix = \"app\")]   — resolve a typed config section\n\
                 \n  #[default]                          — use `Default::default()`",
            ));
        }
    }

    // If any #[config] fields, add R2eConfig to the dependency list once
    if has_config {
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#krate::config::R2eConfig>(), std::any::type_name::<#krate::config::R2eConfig>()) },
        );
        dep_types.push(quote! { #krate::config::R2eConfig });
    }

    let deps_type = build_tcons_type(&dep_types, &krate);

    // Compute BUILD_VERSION from the struct fields tokens
    let build_version = hash_token_stream(&quote! { #fields });

    // Extract R2eConfig once if any #[config] fields are present
    let config_prelude = if has_config {
        quote! { let __r2e_config: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };

    let config_keys_fn = if config_key_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_keys() -> Vec<(&'static str, &'static str)> {
                vec![#(#config_key_entries),*]
            }
        }
    };

    Ok(quote! {
        impl #krate::beans::Bean for #name {
            type Deps = #deps_type;

            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            #config_keys_fn

            const BUILD_VERSION: u64 = #build_version;

            fn build(ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                Self {
                    #(#field_inits,)*
                }
            }
        }
    })
}

