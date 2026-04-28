//! `#[derive(BackgroundService)]` — generates `impl ServiceComponent<State>`.
//!
//! Mirrors the field resolution of `#[derive(Controller)]` (struct-level
//! identity is not supported here — background services have no request
//! context). The user implements an async `run(&self, CancellationToken)`
//! method on the struct; the derive wires `from_state` + `start`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::type_utils::{parse_config_field, parse_config_section_prefix};

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
    let krate = r2e_core_path();

    let mut state_type: Option<syn::Path> = None;
    for attr in &input.attrs {
        if attr.path().is_ident("service") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("state") {
                    let value = meta.value()?;
                    state_type = Some(value.parse()?);
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown attribute in #[service(...)]: expected `state`",
                    ))
                }
            })?;
        }
    }
    let state_type = state_type.ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            "#[service(state = ...)] is required\n\
             example: #[service(state = Services)]",
        )
    })?;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            Fields::Unit => {
                return Ok(generate_unit_impl(name, &state_type, &krate));
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(BackgroundService)] requires named fields or a unit struct",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(BackgroundService)] only works on structs",
            ))
        }
    };

    let mut field_inits: Vec<TokenStream2> = Vec::new();
    let mut has_any_config = false;

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr =
            field.attrs.iter().find(|a| a.path().is_ident("config_section"));

        if is_inject {
            field_inits.push(quote! { #field_name: __state.#field_name.clone() });
        } else if let Some(attr) = config_section_attr {
            let prefix = parse_config_section_prefix(attr)?;
            field_inits.push(quote! {
                #field_name: <#field_type as #krate::ConfigProperties>::from_config(&__cfg, Some(#prefix))
                    .unwrap_or_else(|e| panic!(
                        "Configuration error in `{}`: failed to load section '{}' — {}",
                        #name_str, #prefix, e,
                    ))
            });
            has_any_config = true;
        } else if let Some(attr) = config_attr {
            let (key_str, env_hint, _ty_name) = parse_config_field(attr, field_type)?;
            field_inits.push(quote! {
                #field_name: __cfg.get::<#field_type>(#key_str).unwrap_or_else(|e| panic!(
                    "Configuration error in `{}`: key '{}' — {}. \
                     Add it to application.yaml / application-{{profile}}.yaml, \
                     or set env var `{}`.",
                    #name_str, #key_str, e, #env_hint
                ))
            });
            has_any_config = true;
        } else {
            return Err(syn::Error::new_spanned(
                field_name,
                "background service field must be annotated with one of:\n\
                 \n  #[inject]                           — clone from app state\n\
                 \n  #[config(\"app.key\")]                — resolve from R2eConfig\n\
                 \n  #[config_section(prefix = \"app\")]   — resolve a typed config section",
            ));
        }
    }

    let config_prelude = if has_any_config {
        quote! {
            let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #krate::ServiceComponent<#state_type> for #name {
            fn from_state(__state: &#state_type) -> Self {
                #config_prelude
                Self {
                    #(#field_inits,)*
                }
            }

            fn start(
                self,
                __shutdown: ::tokio_util::sync::CancellationToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    })
}

fn generate_unit_impl(
    name: &syn::Ident,
    state_type: &syn::Path,
    krate: &TokenStream2,
) -> TokenStream2 {
    quote! {
        impl #krate::ServiceComponent<#state_type> for #name {
            fn from_state(_: &#state_type) -> Self { #name }

            fn start(
                self,
                __shutdown: ::tokio_util::sync::CancellationToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    }
}
