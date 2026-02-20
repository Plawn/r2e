use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::type_list_gen::build_tcons_type;

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
    let mut dep_type_ids = Vec::new();
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut field_inits = Vec::new();
    let mut config_key_entries = Vec::new();
    let mut has_config = false;

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));

        if is_inject {
            dep_type_ids.push(quote! { (std::any::TypeId::of::<#field_type>(), std::any::type_name::<#field_type>()) });
            dep_types.push(quote! { #field_type });
            field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
        } else if let Some(attr) = config_attr {
            let key: syn::LitStr = attr.parse_args()?;
            let key_str = key.value();
            let env_hint = key_str.replace('.', "_").to_uppercase();
            let ty_name_str = quote!(#field_type).to_string();
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
        } else {
            // Fields without #[inject] or #[config] use Default::default()
            field_inits.push(quote! { #field_name: Default::default() });
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

            fn build(ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                Self {
                    #(#field_inits,)*
                }
            }
        }
    })
}
