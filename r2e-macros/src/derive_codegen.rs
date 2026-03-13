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

// ── Helpers ─────────────────────────────────────────────────────────────

/// Whether any config or config_section fields exist (requires R2eConfig from state).
fn needs_config(def: &ControllerStructDef) -> bool {
    !def.config_fields.is_empty() || !def.config_section_fields.is_empty()
}

/// Generate inject field initializers that call `StateConstraint` methods.
fn inject_init_via_constraint(
    def: &ControllerStructDef,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    def.injected_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let method = format_ident!("__inject_{}", field_name);
            quote! { #field_name: <() as #meta_mod::StateConstraint<__S>>::#method(__state) }
        })
        .collect()
}

/// Generate identity field extractions that call `StateConstraint` methods.
fn identity_extractions_via_constraint(
    def: &ControllerStructDef,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    def.identity_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let method = format_ident!("__extract_{}", field_name);
            quote! {
                let #field_name = <() as #meta_mod::StateConstraint<__S>>::#method(__parts, __state).await?;
            }
        })
        .collect()
}

/// Generate config field initializers that call `StateConstraint::__get_config`.
fn config_init_via_constraint(
    def: &ControllerStructDef,
    meta_mod: &syn::Ident,
    krate: &TokenStream,
    panic_on_error: bool,
) -> Vec<TokenStream> {
    let controller_name_str = def.name.to_string();
    def.config_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key = &f.key;
            let env_hint = key.to_uppercase().replace('.', "_");
            if panic_on_error {
                quote! {
                    #field_name: {
                        let __cfg = <() as #meta_mod::StateConstraint<__S>>::__get_config(__state);
                        __cfg.get(#key).unwrap_or_else(|e| panic!(
                            "Configuration error in `{}`: key '{}' — {}. \
                             Add it to application.yaml / application-{{profile}}.yaml, \
                             or set env var `{}`.",
                            #controller_name_str, #key, e, #env_hint
                        ))
                    }
                }
            } else {
                quote! {
                    #field_name: {
                        let __cfg = <() as #meta_mod::StateConstraint<__S>>::__get_config(__state);
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
            }
        })
        .collect()
}

/// Generate config section field initializers that call `StateConstraint::__get_config`.
fn config_section_init_via_constraint(
    def: &ControllerStructDef,
    meta_mod: &syn::Ident,
    krate: &TokenStream,
    panic_on_error: bool,
) -> Vec<TokenStream> {
    let controller_name_str = def.name.to_string();
    def.config_section_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_type = &f.ty;
            let prefix = &f.prefix;
            if panic_on_error {
                quote! {
                    #field_name: {
                        let __cfg = <() as #meta_mod::StateConstraint<__S>>::__get_config(__state);
                        <#field_type as #krate::ConfigProperties>::from_config(&__cfg, Some(#prefix))
                            .unwrap_or_else(|e| panic!(
                                "Configuration error in `{}`: failed to load section '{}' — {}",
                                #controller_name_str,
                                #prefix,
                                e,
                            ))
                    }
                }
            } else {
                quote! {
                    #field_name: {
                        let __cfg = <() as #meta_mod::StateConstraint<__S>>::__get_config(__state);
                        match <#field_type as #krate::ConfigProperties>::from_config(&__cfg, Some(#prefix)) {
                            Ok(v) => v,
                            Err(e) => {
                                return Err(#krate::http::response::IntoResponse::into_response(
                                    #krate::HttpError::Internal(
                                        format!(
                                            "Configuration error in {}: failed to load section '{}' — {}",
                                            #controller_name_str,
                                            #prefix,
                                            e,
                                        )
                                    )
                                ));
                            }
                        }
                    }
                }
            }
        })
        .collect()
}

// ── Meta module generation ──────────────────────────────────────────────

/// Generate `mod __r2e_meta_<Name>` with `StateConstraint<__S>` trait,
/// PATH_PREFIX, IdentityType, and guard_identity.
fn generate_meta_module(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
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

    let config_section_validations: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| {
            let field_type = &f.ty;
            let prefix = &f.prefix;
            quote! {
                __errors.extend(#krate::config::validation::validate_section::<#field_type>(__config, Some(#prefix)));
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

    // ── StateConstraint<__S> trait ─────────────────────────────────────
    //
    // Implemented on `()`. Provides methods for extracting fields from state.
    // This avoids the Rust limitation where trait `where` clauses don't
    // create implied bounds for trait users.
    //
    // When `state = X`: concrete impl for X only.
    // When state is omitted: blanket impl with FromRef/FromRequestParts bounds.

    let state_constraint = generate_state_constraint_trait(def, &krate);

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        mod #mod_name {
            use super::*;
            pub const PATH_PREFIX: Option<&str> = #path_prefix;
            pub const HAS_STRUCT_IDENTITY: bool = #has_struct_identity;
            #identity_type
            #guard_identity_fn
            #validate_fn
            #state_constraint
        }
    }
}

/// Generate the `StateConstraint<__S>` trait with methods for field access.
fn generate_state_constraint_trait(def: &ControllerStructDef, krate: &TokenStream) -> TokenStream {
    // ── Trait methods ──

    // Inject methods: fn __inject_<field_name>(state: &__S) -> FieldType
    let inject_method_sigs: Vec<TokenStream> = def
        .injected_fields
        .iter()
        .map(|f| {
            let method = format_ident!("__inject_{}", f.name);
            let field_type = &f.ty;
            quote! { fn #method(__state: &__S) -> #field_type; }
        })
        .collect();

    // Identity methods: async extraction via RPITIT
    let identity_method_sigs: Vec<TokenStream> = def
        .identity_fields
        .iter()
        .map(|f| {
            let method = format_ident!("__extract_{}", f.name);
            let field_type = &f.ty;
            quote! {
                fn #method(
                    __parts: &mut #krate::http::header::Parts,
                    __state: &__S,
                ) -> impl std::future::Future<Output = Result<#field_type, #krate::http::response::Response>> + Send;
            }
        })
        .collect();

    // Config method (if any config fields exist)
    let config_method_sig = if needs_config(def) {
        quote! { fn __get_config(__state: &__S) -> #krate::R2eConfig; }
    } else {
        quote! {}
    };

    // ── Inject method impls ──

    // Closed case: use direct field access (state.field_name.clone())
    // Open case: use FromRef (requires FromRef bounds)
    let inject_method_impls_closed = |param_ty: &TokenStream| -> Vec<TokenStream> {
        def.injected_fields
            .iter()
            .map(|f| {
                let method = format_ident!("__inject_{}", f.name);
                let field_name = &f.name;
                let field_type = &f.ty;
                quote! {
                    fn #method(__state: &#param_ty) -> #field_type {
                        __state.#field_name.clone()
                    }
                }
            })
            .collect()
    };

    let inject_method_impls_open = || -> Vec<TokenStream> {
        def.injected_fields
            .iter()
            .map(|f| {
                let method = format_ident!("__inject_{}", f.name);
                let field_type = &f.ty;
                quote! {
                    fn #method(__state: &__S) -> #field_type {
                        <#field_type as #krate::http::extract::FromRef<__S>>::from_ref(__state)
                    }
                }
            })
            .collect()
    };

    let identity_method_impls = |param_ty: &TokenStream, from_req_ty: &TokenStream| -> Vec<TokenStream> {
        def.identity_fields
            .iter()
            .map(|f| {
                let method = format_ident!("__extract_{}", f.name);
                let field_type = &f.ty;
                quote! {
                    async fn #method(
                        __parts: &mut #krate::http::header::Parts,
                        __state: &#param_ty,
                    ) -> Result<#field_type, #krate::http::response::Response> {
                        <#field_type as #krate::http::extract::FromRequestParts<#from_req_ty>>
                            ::from_request_parts(__parts, __state)
                            .await
                            .map_err(#krate::http::response::IntoResponse::into_response)
                    }
                }
            })
            .collect()
    };

    let config_method_impl = |param_ty: &TokenStream, from_ref_ty: &TokenStream| -> TokenStream {
        if !needs_config(def) {
            return quote! {};
        }
        quote! {
            fn __get_config(__state: &#param_ty) -> #krate::R2eConfig {
                <#krate::R2eConfig as #krate::http::extract::FromRef<#from_ref_ty>>::from_ref(__state)
            }
        }
    };

    // ── Generate closed or open impl ──

    match &def.state_type {
        Some(state_type) => {
            // Closed: concrete impl for the specified state type only.
            // Also generate `type State = X;` for handler/controller codegen.
            let state_ty = quote! { #state_type };
            let inj_impls = inject_method_impls_closed(&state_ty);
            let id_impls = identity_method_impls(&state_ty, &state_ty);
            let cfg_impl = config_method_impl(&state_ty, &state_ty);

            quote! {
                pub type State = #state_type;

                pub trait StateConstraint<__S: Clone + Send + Sync + 'static> {
                    #(#inject_method_sigs)*
                    #(#identity_method_sigs)*
                    #config_method_sig
                }

                impl StateConstraint<#state_type> for () {
                    #(#inj_impls)*
                    #(#id_impls)*
                    #cfg_impl
                }
            }
        }
        None => {
            // Open: blanket impl for any type satisfying the bounds.
            let s_ty = quote! { __S };
            let inj_impls = inject_method_impls_open();
            let id_impls = identity_method_impls(&s_ty, &s_ty);
            let cfg_impl = config_method_impl(&s_ty, &s_ty);

            // Collect where clause bounds for the blanket impl
            let inject_bounds: Vec<TokenStream> = def
                .injected_fields
                .iter()
                .map(|f| {
                    let field_type = &f.ty;
                    quote! { #field_type: #krate::http::extract::FromRef<__S> }
                })
                .collect();
            let identity_bounds: Vec<TokenStream> = def
                .identity_fields
                .iter()
                .flat_map(|f| {
                    let field_type = &f.ty;
                    vec![
                        quote! { #field_type: #krate::http::extract::FromRequestParts<__S> },
                        quote! { <#field_type as #krate::http::extract::FromRequestParts<__S>>::Rejection: #krate::http::response::IntoResponse },
                    ]
                })
                .collect();
            let config_bounds = if needs_config(def) {
                vec![quote! { #krate::R2eConfig: #krate::http::extract::FromRef<__S> }]
            } else {
                vec![]
            };

            quote! {
                pub trait StateConstraint<__S: Clone + Send + Sync + 'static> {
                    #(#inject_method_sigs)*
                    #(#identity_method_sigs)*
                    #config_method_sig
                }

                impl<__S> StateConstraint<__S> for ()
                where
                    __S: Clone + Send + Sync + 'static,
                    #(#inject_bounds,)*
                    #(#identity_bounds,)*
                    #(#config_bounds,)*
                {
                    #(#inj_impls)*
                    #(#id_impls)*
                    #cfg_impl
                }
            }
        }
    }
}

// ── Extractor generation ────────────────────────────────────────────────

/// Generate `struct __R2eExtract_<Name>` + `impl FromRequestParts<__S>`.
fn generate_extractor(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
    let meta_mod = format_ident!("__r2e_meta_{}", name);
    let extractor_name = format_ident!("__R2eExtract_{}", name);

    // Identity extractions via StateConstraint methods
    let identity_extractions = identity_extractions_via_constraint(def, &meta_mod);

    // Inject field initializers via StateConstraint methods
    let inject_inits = inject_init_via_constraint(def, &meta_mod);

    // Identity field initializers (already extracted above)
    let identity_inits: Vec<TokenStream> = def
        .identity_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            quote! { #field_name: #field_name }
        })
        .collect();

    // Config + config section inits via StateConstraint (return Err on failure)
    let config_inits = config_init_via_constraint(def, &meta_mod, &krate, false);
    let config_section_inits = config_section_init_via_constraint(def, &meta_mod, &krate, false);

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

        impl<__S> #krate::http::extract::FromRequestParts<__S> for #extractor_name
        where
            __S: Clone + Send + Sync + 'static,
            (): #meta_mod::StateConstraint<__S>,
        {
            type Rejection = #krate::http::response::Response;

            async fn from_request_parts(
                __parts: &mut #krate::http::header::Parts,
                __state: &__S,
            ) -> Result<Self, Self::Rejection> {
                #(#identity_extractions)*
                Ok(Self(#struct_init))
            }
        }
    }
}

// ── StatefulConstruct generation ────────────────────────────────────────

/// Generate `impl StatefulConstruct<__S> for Name` when there are no identity fields.
fn generate_stateful_construct(def: &ControllerStructDef) -> TokenStream {
    if !def.identity_fields.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();
    let name = &def.name;
    let meta_mod = format_ident!("__r2e_meta_{}", name);

    // Inject field initializers via StateConstraint methods
    let inject_inits = inject_init_via_constraint(def, &meta_mod);

    // Config + config section inits via StateConstraint (panic on failure)
    let config_inits = config_init_via_constraint(def, &meta_mod, &krate, true);
    let config_section_inits = config_section_init_via_constraint(def, &meta_mod, &krate, true);

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
        impl<__S> #krate::StatefulConstruct<__S> for #name
        where
            __S: Clone + Send + Sync + 'static,
            (): #meta_mod::StateConstraint<__S>,
        {
            fn from_state(__state: &__S) -> Self {
                #struct_init
            }
        }
    }
}
