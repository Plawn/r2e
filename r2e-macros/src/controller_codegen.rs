use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::controller_parsing::ControllerStructDef;
use crate::crate_path::r2e_core_path;
use crate::field_resolver::{config_init_panic, config_section_init_panic};

/// Generate the physical controller core plus all supporting items.
///
/// `physical_struct` is the user's struct with the controller helper field
/// attributes stripped AND every request-scoped field (identity +
/// `#[inject(request)]`) removed — those move onto the generated request façade.
///
/// Emits, per controller:
/// - the physical core struct (app/config fields only);
/// - `mod __r2e_meta_<Name>` (state alias, path prefix, identity metadata,
///   `guard_identity`, `bind_request`, `build_routes`, config validation);
/// - `struct __R2eRequestData_<Name>` + `FromRequestParts` (request-scoped values);
/// - `struct __R2eRequest_<Name>` + `Deref<Target = Core>` (the request façade);
/// - `impl StatefulConstruct<State>` for the core (always — the core has no
///   request-scoped fields, so it is always buildable from state).
pub fn generate(def: &ControllerStructDef, physical_struct: &syn::ItemStruct) -> TokenStream {
    let meta_module = generate_meta_module(def);
    let request_data = generate_request_data(def);
    let facade = generate_facade(def);
    let stateful_construct = generate_stateful_construct(def);

    quote! {
        #physical_struct
        #meta_module
        #request_data
        #facade
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

    let facade_name = format_ident!("__R2eRequest_{}", name);
    let data_name = format_ident!("__R2eRequestData_{}", name);

    // `guard_identity` reads the identity field off the request façade, never the
    // core (which no longer holds it). Non-identity `#[inject(request)]` fields
    // are not visible to guards.
    let (identity_type, guard_identity_fn) = if let Some(identity) = def.identity_fields.first() {
        let field_name = &identity.name;
        let inner_ty = &identity.inner_ty;
        let identity_expr = if identity.is_optional {
            quote! { __facade.#field_name.as_ref() }
        } else {
            quote! { Some(&__facade.#field_name) }
        };
        (
            quote! { pub type IdentityType = #inner_ty; },
            quote! {
                pub fn guard_identity(__facade: &super::#facade_name) -> Option<&super::#inner_ty> {
                    #identity_expr
                }
            },
        )
    } else {
        (
            quote! { pub type IdentityType = #krate::NoIdentity; },
            quote! {
                pub fn guard_identity(_facade: &super::#facade_name) -> Option<&#krate::NoIdentity> {
                    None
                }
            },
        )
    };

    // Move request-scoped values from the extracted request-data type onto the
    // façade, which also owns an `Arc` to the application core. Generated here
    // (by the struct macro) so `#[routes]` never needs to know the façade fields.
    let rs_field_names: Vec<&syn::Ident> = def
        .request_scoped_fields()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let bind_request_fn = quote! {
        #[inline]
        pub fn bind_request(
            __core: ::std::sync::Arc<super::#name>,
            __data: super::#data_name,
        ) -> super::#facade_name {
            super::#facade_name {
                __core,
                #( #rs_field_names: __data.#rs_field_names, )*
            }
        }
    };

    // Generate unified validate_config function
    let controller_name_str = name.to_string();

    let config_key_entries: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| {
            let key = &f.key;
            let ty_name = &f.ty_name;
            quote! { (#controller_name_str, #key, #ty_name) }
        })
        .collect();

    // #[config_section] fields
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

    // Single, uniform route-construction path: the core always implements
    // `StatefulConstruct` (it holds no request-scoped fields), so it is built
    // once into an `Arc` here and handed to the application router. Per request,
    // the route closures extract `__R2eRequestData_<Name>` and bind the façade —
    // there is no longer a request-vs-application branch.
    let build_routes_fn = quote! {
        pub fn build_routes<F>(
            __state: &State,
            __application_router: F,
        ) -> #krate::http::Router<State>
        where
            F: ::core::ops::FnOnce(::std::sync::Arc<super::#name>) -> #krate::http::Router<State>,
        {
            let __controller = ::std::sync::Arc::new(
                <super::#name as #krate::StatefulConstruct<State>>::from_state(__state)
            );
            __application_router(__controller)
        }
    };

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
            #bind_request_fn
            #build_routes_fn
            #validate_fn
        }
    }
}

/// Generate `struct __R2eRequestData_<Name>` + `impl FromRequestParts<State>`.
///
/// Holds every request-scoped field (identity + `#[inject(request)]`), each
/// extracted via its own `FromRequestParts`. For controllers with no
/// request-scoped fields this is a zero-sized type with an infallible extractor,
/// so the single dispatch path compiles to a no-op there.
///
/// NOTE (OpenAPI): `#[inject(request)]` fields are intentionally NOT modeled in
/// the generated OpenAPI spec in this pass. Only identity drives the security
/// requirement (via `HAS_STRUCT_IDENTITY`); modeling request fields as
/// parameters or security schemes is deferred.
fn generate_request_data(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
    let state_type = &def.state_type;
    let data_name = format_ident!("__R2eRequestData_{}", name);

    let fields = def.request_scoped_fields();

    if fields.is_empty() {
        return quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            struct #data_name;

            impl #krate::http::extract::FromRequestParts<#state_type> for #data_name {
                type Rejection = #krate::http::response::Response;

                async fn from_request_parts(
                    _parts: &mut #krate::http::header::Parts,
                    _state: &#state_type,
                ) -> Result<Self, Self::Rejection> {
                    Ok(#data_name)
                }
            }
        };
    }

    let field_decls: Vec<TokenStream> = fields
        .iter()
        .map(|(field_name, field_ty)| quote! { #field_name: #field_ty })
        .collect();

    let extractions: Vec<TokenStream> = fields
        .iter()
        .map(|(field_name, field_ty)| {
            quote! {
                let #field_name = <#field_ty as #krate::http::extract::FromRequestParts<#state_type>>
                    ::from_request_parts(__parts, __state)
                    .await
                    .map_err(#krate::http::response::IntoResponse::into_response)?;
            }
        })
        .collect();

    let field_inits: Vec<&syn::Ident> = fields.iter().map(|(n, _)| *n).collect();

    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        struct #data_name {
            #(#field_decls,)*
        }

        impl #krate::http::extract::FromRequestParts<#state_type> for #data_name {
            type Rejection = #krate::http::response::Response;

            async fn from_request_parts(
                __parts: &mut #krate::http::header::Parts,
                __state: &#state_type,
            ) -> Result<Self, Self::Rejection> {
                #(#extractions)*
                Ok(Self { #(#field_inits),* })
            }
        }
    }
}

/// Generate the request façade `struct __R2eRequest_<Name>` + `Deref<Target = Core>`.
///
/// The façade owns an `Arc` to the application core plus every request-scoped
/// value, all by value (never borrowed), so it is `Send + Sync` whenever its
/// fields are and stays alive for the whole SSE/WS future. Route bodies run on
/// this type: `self.<identity/request field>` resolves to a façade field, while
/// `self.<injected/config field>` and core helpers resolve through `Deref`.
fn generate_facade(def: &ControllerStructDef) -> TokenStream {
    let name = &def.name;
    let facade_name = format_ident!("__R2eRequest_{}", name);

    let field_decls: Vec<TokenStream> = def
        .request_scoped_fields()
        .iter()
        .map(|(field_name, field_ty)| quote! { #field_name: #field_ty })
        .collect();

    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        struct #facade_name {
            __core: ::std::sync::Arc<#name>,
            #(#field_decls,)*
        }

        impl ::core::ops::Deref for #facade_name {
            type Target = #name;

            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.__core
            }
        }
    }
}

/// Generate `impl StatefulConstruct<State> for Name`.
///
/// Always emitted: the physical core holds only app/config-scoped fields (every
/// request-scoped field moved to the façade), so it is always buildable from
/// state. This is what lets `build_routes` use one uniform Arc-capture path and
/// lets consumers/scheduled tasks reconstruct the core off-request.
fn generate_stateful_construct(def: &ControllerStructDef) -> TokenStream {
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

    let controller_name_str = name.to_string();
    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| config_init_panic(&f.name, &f.key, &f.env_hint, &controller_name_str))
        .collect();

    let config_section_inits: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| config_section_init_panic(&f.name, &f.ty, &f.prefix, &controller_name_str, &krate))
        .collect();

    let has_any_config = !def.config_fields.is_empty() || !def.config_section_fields.is_empty();
    let config_prelude = if has_any_config {
        quote! {
            let __cfg = <#krate::R2eConfig as #krate::http::extract::FromRef<#state_type>>::from_ref(__state);
        }
    } else {
        quote! {}
    };

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
                #config_prelude
                #struct_init
            }
        }
    }
}
