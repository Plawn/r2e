use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::derive_parsing::ControllerStructDef;

pub fn generate(def: &ControllerStructDef) -> TokenStream {
    let meta_module = generate_meta_module(def);
    let extractor = generate_extractor(def);
    let stateful_construct = generate_stateful_construct(def);

    quote! {
        #meta_module
        #extractor
        #stateful_construct
    }
}

/// Generate `mod __quarlus_meta_<Name>` with State type alias, PATH_PREFIX,
/// IdentityType, and guard_identity.
fn generate_meta_module(def: &ControllerStructDef) -> TokenStream {
    let name = &def.name;
    let state_type = &def.state_type;
    let mod_name = format_ident!("__quarlus_meta_{}", name);

    let path_prefix = match &def.prefix {
        Some(p) => quote! { Some(#p) },
        None => quote! { None },
    };

    let (identity_type, guard_identity_fn) = if let Some(identity) = def.identity_fields.first() {
        let field_name = &identity.name;
        let field_type = &identity.ty;
        (
            quote! { pub type IdentityType = #field_type; },
            quote! {
                pub fn guard_identity(ctrl: &super::#name) -> Option<&super::#field_type> {
                    Some(&ctrl.#field_name)
                }
            },
        )
    } else {
        (
            quote! { pub type IdentityType = quarlus_core::NoIdentity; },
            quote! {
                pub fn guard_identity(_ctrl: &super::#name) -> Option<&quarlus_core::NoIdentity> {
                    None
                }
            },
        )
    };

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        mod #mod_name {
            use super::*;
            pub type State = #state_type;
            pub const PATH_PREFIX: Option<&str> = #path_prefix;
            #identity_type
            #guard_identity_fn
        }
    }
}

/// Generate `struct __QuarlusExtract_<Name>` + `impl FromRequestParts<State>`.
fn generate_extractor(def: &ControllerStructDef) -> TokenStream {
    let name = &def.name;
    let state_type = &def.state_type;
    let extractor_name = format_ident!("__QuarlusExtract_{}", name);

    // Identity extractions (async, from request parts)
    let identity_extractions: Vec<TokenStream> = def
        .identity_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_type = &f.ty;
            quote! {
                let #field_name = <#field_type as quarlus_core::http::extract::FromRequestParts<#state_type>>
                    ::from_request_parts(__parts, __state)
                    .await
                    .map_err(quarlus_core::http::response::IntoResponse::into_response)?;
            }
        })
        .collect();

    // Inject field initializers (cloned from state)
    let inject_inits: Vec<TokenStream> = def
        .injected_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            quote! { #field_name: __state.#field_name.clone() }
        })
        .collect();

    // Identity field initializers (already extracted above)
    let identity_inits: Vec<TokenStream> = def
        .identity_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            quote! { #field_name: #field_name }
        })
        .collect();

    // Config field initializers
    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key = &f.key;
            quote! {
                #field_name: {
                    let __cfg = <quarlus_core::QuarlusConfig as quarlus_core::http::extract::FromRef<#state_type>>::from_ref(__state);
                    __cfg.get(#key).unwrap_or_else(|e| panic!("Config key '{}' error: {}", #key, e))
                }
            }
        })
        .collect();

    // All field initializers in declaration order
    let all_inits: Vec<&TokenStream> = inject_inits
        .iter()
        .chain(identity_inits.iter())
        .chain(config_inits.iter())
        .collect();

    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        pub struct #extractor_name(pub #name);

        impl quarlus_core::http::extract::FromRequestParts<#state_type> for #extractor_name {
            type Rejection = quarlus_core::http::response::Response;

            async fn from_request_parts(
                __parts: &mut quarlus_core::http::header::Parts,
                __state: &#state_type,
            ) -> Result<Self, Self::Rejection> {
                #(#identity_extractions)*
                Ok(Self(#name {
                    #(#all_inits,)*
                }))
            }
        }
    }
}

/// Generate `impl StatefulConstruct<State> for Name` when there are no identity fields.
fn generate_stateful_construct(def: &ControllerStructDef) -> TokenStream {
    if !def.identity_fields.is_empty() {
        return quote! {};
    }

    let name = &def.name;
    let state_type = &def.state_type;

    let inject_inits: Vec<TokenStream> = def
        .injected_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            quote! { #field_name: __state.#field_name.clone() }
        })
        .collect();

    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key = &f.key;
            quote! {
                #field_name: {
                    let __cfg = <quarlus_core::QuarlusConfig as quarlus_core::http::extract::FromRef<#state_type>>::from_ref(__state);
                    __cfg.get(#key).unwrap_or_else(|e| panic!("Config key '{}' error: {}", #key, e))
                }
            }
        })
        .collect();

    let all_inits: Vec<&TokenStream> = inject_inits
        .iter()
        .chain(config_inits.iter())
        .collect();

    quote! {
        impl quarlus_core::StatefulConstruct<#state_type> for #name {
            fn from_state(__state: &#state_type) -> Self {
                Self {
                    #(#all_inits,)*
                }
            }
        }
    }
}
