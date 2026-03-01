use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::r2e_core_path;
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

/// Generate `mod __r2e_meta_<Name>` with State type alias, PATH_PREFIX,
/// IdentityType, and guard_identity.
fn generate_meta_module(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
    let state_type = &def.state_type;
    let mod_name = format_ident!("__r2e_meta_{}", name);

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
            quote! { pub type IdentityType = #krate::NoIdentity; },
            quote! {
                pub fn guard_identity(_ctrl: &super::#name) -> Option<&#krate::NoIdentity> {
                    None
                }
            },
        )
    };

    // Generate unified validate_config function
    let controller_name_str = name.to_string();

    // Individual #[config("key")] fields
    let config_key_entries: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let key = &f.key;
            let ty = &f.ty;
            let ty_name_str = quote!(#ty).to_string();
            quote! { (#controller_name_str, #key, #ty_name_str) }
        })
        .collect();

    // #[config_section] fields
    let config_section_validations: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| {
            let field_type = &f.ty;
            quote! {
                __errors.extend(#krate::config::validation::validate_section::<#field_type>(__config));
            }
        })
        .collect();

    let has_any_config = !config_key_entries.is_empty() || !config_section_validations.is_empty();

    let validate_fn = if !has_any_config {
        quote! {
            pub fn validate_config(
                _config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                Vec::new()
            }
        }
    } else {
        let keys_validation = if config_key_entries.is_empty() {
            quote! {}
        } else {
            quote! {
                let __keys: &[(&str, &str, &str)] = &[#(#config_key_entries),*];
                __errors.extend(#krate::config::validation::validate_keys(__config, __keys));
            }
        };

        quote! {
            pub fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                let mut __errors = Vec::new();
                #keys_validation
                #(#config_section_validations)*
                __errors
            }
        }
    };

    let has_struct_identity = !def.identity_fields.is_empty();

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        mod #mod_name {
            use super::*;
            pub type State = #state_type;
            pub const PATH_PREFIX: Option<&str> = #path_prefix;
            pub const HAS_STRUCT_IDENTITY: bool = #has_struct_identity;
            #identity_type
            #guard_identity_fn
            #validate_fn
        }
    }
}

/// Generate `struct __R2eExtract_<Name>` + `impl FromRequestParts<State>`.
fn generate_extractor(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
    let state_type = &def.state_type;
    let extractor_name = format_ident!("__R2eExtract_{}", name);

    // Identity extractions (async, from request parts)
    let identity_extractions: Vec<TokenStream> = def
        .identity_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_type = &f.ty;
            quote! {
                let #field_name = <#field_type as #krate::http::extract::FromRequestParts<#state_type>>
                    ::from_request_parts(__parts, __state)
                    .await
                    .map_err(#krate::http::response::IntoResponse::into_response)?;
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
    let controller_name_str = name.to_string();
    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key = &f.key;
            let env_hint = key.to_uppercase().replace('.', "_");
            quote! {
                #field_name: {
                    let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
                    match __cfg.get(#key) {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(#krate::http::response::IntoResponse::into_response(
                                #krate::HttpError::Internal(
                                    format!(
                                        "Configuration error in {}: key '{}' — {}. \
                                         Add it to application.yaml or set env var {}.",
                                        #controller_name_str, #key, e, #env_hint
                                    )
                                )
                            ));
                        }
                    }
                }
            }
        })
        .collect();

    // Config section field initializers (#[config_section])
    let config_section_inits: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_type = &f.ty;
            quote! {
                #field_name: {
                    let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
                    match <#field_type as #krate::ConfigProperties>::from_config(&__cfg) {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(#krate::http::response::IntoResponse::into_response(
                                #krate::HttpError::Internal(
                                    format!(
                                        "Configuration error in {}: failed to load {} — {}",
                                        #controller_name_str,
                                        <#field_type as #krate::ConfigProperties>::prefix(),
                                        e,
                                    )
                                )
                            ));
                        }
                    }
                }
            }
        })
        .collect();

    // All field initializers in declaration order
    let all_inits: Vec<&TokenStream> = inject_inits
        .iter()
        .chain(identity_inits.iter())
        .chain(config_inits.iter())
        .chain(config_section_inits.iter())
        .collect();

    let struct_init = if def.is_unit_struct {
        quote! { #name }
    } else {
        quote! { #name { #(#all_inits,)* } }
    };

    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        pub struct #extractor_name(pub #name);

        impl #krate::http::extract::FromRequestParts<#state_type> for #extractor_name {
            type Rejection = #krate::http::response::Response;

            async fn from_request_parts(
                __parts: &mut #krate::http::header::Parts,
                __state: &#state_type,
            ) -> Result<Self, Self::Rejection> {
                #(#identity_extractions)*
                Ok(Self(#struct_init))
            }
        }
    }
}

/// Generate `impl StatefulConstruct<State> for Name` when there are no identity fields.
fn generate_stateful_construct(def: &ControllerStructDef) -> TokenStream {
    if !def.identity_fields.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();
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

    let sc_controller_name_str = name.to_string();
    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key = &f.key;
            let env_hint = key.to_uppercase().replace('.', "_");
            quote! {
                #field_name: {
                    let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
                    __cfg.get(#key).unwrap_or_else(|e| panic!(
                        "Configuration error in `{}`: key '{}' — {}. \
                         Add it to application.yaml / application-{{profile}}.yaml, \
                         or set env var `{}`.",
                        #sc_controller_name_str, #key, e, #env_hint
                    ))
                }
            }
        })
        .collect();

    let config_section_inits: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_type = &f.ty;
            quote! {
                #field_name: {
                    let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
                    <#field_type as #krate::ConfigProperties>::from_config(&__cfg)
                        .unwrap_or_else(|e| panic!(
                            "Configuration error in `{}`: failed to load {} — {}",
                            #sc_controller_name_str,
                            <#field_type as #krate::ConfigProperties>::prefix(),
                            e,
                        ))
                }
            }
        })
        .collect();

    let all_inits: Vec<&TokenStream> = inject_inits
        .iter()
        .chain(config_inits.iter())
        .chain(config_section_inits.iter())
        .collect();

    let struct_init = if def.is_unit_struct {
        quote! { #name }
    } else {
        quote! { Self { #(#all_inits,)* } }
    };

    quote! {
        impl #krate::StatefulConstruct<#state_type> for #name {
            fn from_state(__state: &#state_type) -> Self {
                #struct_init
            }
        }
    }
}
