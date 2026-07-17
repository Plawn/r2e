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
/// - `mod __r2e_meta_<Name>` (path prefix, identity metadata, `guard_identity`,
///   `bind_request`, config validation; plus a `State` alias on the legacy
///   named-state path);
/// - `struct __R2eRequestData_<Name><__M>` + a state-generic
///   `FromRequestParts` impl (request-scoped values, extracted through
///   `FromRequestPartsVia` with the per-field markers folded into `__M`);
/// - `struct __R2eRequest_<Name>` + `Deref<Target = Core>` (the request façade);
/// - `impl ContextConstruct` for the core (always — cores are built by type
///   from the resolved bean graph).
pub fn generate(def: &ControllerStructDef, physical_struct: &syn::ItemStruct) -> TokenStream {
    let physical_struct = add_deco_slot_field(physical_struct.clone());
    let meta_module = generate_meta_module(def);
    let request_data = generate_request_data(def);
    let facade = generate_facade(def);
    let context_construct = generate_context_construct(def);

    quote! {
        #physical_struct
        #meta_module
        #request_data
        #facade
        #context_construct
    }
}

/// Add the hidden `__r2e_decos: DecoSlot` field to the physical core.
///
/// The slot carries the prebuilt scheduled-method decorator sets so the
/// method bodies (which only have `&self`) can run their interceptor chain on
/// direct calls too — `scheduled_tasks_boxed` fills it at registration. Every
/// core gets the field (`#[routes]` can rely on it unconditionally); a unit
/// struct becomes a named struct holding just the slot. `DecoSlot` implements
/// `Clone`/`Debug`/`Default` so user derives keep working; cores are no
/// longer literal-constructible (use `ContextConstruct::from_context`).
fn add_deco_slot_field(mut item: syn::ItemStruct) -> syn::ItemStruct {
    let krate = r2e_core_path();
    let fields_named: syn::FieldsNamed = syn::parse_quote!({
        #[doc(hidden)]
        __r2e_decos: #krate::decorator::DecoSlot
    });
    match &mut item.fields {
        syn::Fields::Named(named) => {
            named.named.extend(fields_named.named);
        }
        syn::Fields::Unit => {
            item.fields = syn::Fields::Named(fields_named);
        }
        // Tuple-struct controllers are rejected at parse time.
        syn::Fields::Unnamed(_) => {}
    }
    item
}

/// Generate `mod __r2e_meta_<Name>` with PATH_PREFIX, IdentityType,
/// guard_identity, bind_request, and config validation.
fn generate_meta_module(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
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
    // Generic over the data marker `__M` — `#[routes]` threads it opaquely.
    let rs_field_names: Vec<&syn::Ident> = def
        .request_scoped_fields()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let bind_request_fn = quote! {
        #[inline]
        pub fn bind_request<__M>(
            __core: ::std::sync::Arc<super::#name>,
            __data: super::#data_name<__M>,
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
        // `Option<T>` config fields are optional — an absent key is legal, so
        // it must NOT be reported as a missing required key.
        .filter(|f| !f.is_option)
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
    // `#[anonymous]` (checked by `#[routes]` via const-assert) only makes sense
    // against a fail-closed baseline: a *required* struct identity. An
    // `Option<T>` identity never rejects, so there is nothing to opt out of.
    let struct_identity_is_required = def
        .identity_fields
        .first()
        .is_some_and(|f| !f.is_optional);

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        mod #mod_name {
            use super::*;
            pub const PATH_PREFIX: Option<&str> = #path_prefix;
            pub const HAS_STRUCT_IDENTITY: bool = #has_struct_identity;
            pub const STRUCT_IDENTITY_IS_REQUIRED: bool = #struct_identity_is_required;
            #identity_type
            #guard_identity_fn
            #bind_request_fn
            #validate_fn
        }
    }
}

/// Generate `struct __R2eRequestData_<Name><__M>` + a state-generic
/// `impl FromRequestParts<S>`.
///
/// Holds every request-scoped field (identity + `#[inject(request)]`), each
/// extracted through [`FromRequestPartsVia`] so bean-backed extractors can
/// park their `HasBean` index witnesses in the marker. The single `__M`
/// parameter is the tuple of per-field markers — `#[routes]` threads it as one
/// opaque generic without knowing the field count; the impl here (which knows
/// the fields) destructures it.
///
/// For controllers with no request-scoped fields the struct is marker-only and
/// the extractor (implemented for `__M = ()`) is infallible, so the single
/// dispatch path compiles to a no-op there.
///
/// NOTE (OpenAPI): `#[inject(request)]` fields are intentionally NOT modeled in
/// the generated OpenAPI spec in this pass. Only identity drives the security
/// requirement (via `HAS_STRUCT_IDENTITY`); modeling request fields as
/// parameters or security schemes is deferred.
fn generate_request_data(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;
    let data_name = format_ident!("__R2eRequestData_{}", name);

    let fields = def.request_scoped_fields();

    if fields.is_empty() {
        return quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            struct #data_name<__M> {
                __r2e_markers: ::std::marker::PhantomData<fn() -> __M>,
            }

            impl<__R2eS> #krate::http::extract::FromRequestParts<__R2eS> for #data_name<()>
            where
                __R2eS: Send + Sync,
            {
                type Rejection = ::std::convert::Infallible;

                #[inline(always)]
                async fn from_request_parts(
                    _parts: &mut #krate::http::header::Parts,
                    _state: &__R2eS,
                ) -> Result<Self, Self::Rejection> {
                    Ok(#data_name {
                        __r2e_markers: ::std::marker::PhantomData,
                    })
                }
            }
        };
    }

    let field_decls: Vec<TokenStream> = fields
        .iter()
        .map(|(field_name, field_ty)| quote! { #field_name: #field_ty })
        .collect();

    let marker_idents: Vec<syn::Ident> = (0..fields.len())
        .map(|i| format_ident!("__M{}", i))
        .collect();

    let extractions: Vec<TokenStream> = fields
        .iter()
        .zip(marker_idents.iter())
        .map(|((field_name, field_ty), marker)| {
            quote! {
                let #field_name = <#field_ty as #krate::extract::FromRequestPartsVia<__R2eS, #marker>>
                    ::from_request_parts_via(__parts, __state)
                    .await
                    .map_err(#krate::http::response::IntoResponse::into_response)?;
            }
        })
        .collect();

    let via_bounds: Vec<TokenStream> = fields
        .iter()
        .zip(marker_idents.iter())
        .map(|((_, field_ty), marker)| {
            quote! { #field_ty: #krate::extract::FromRequestPartsVia<__R2eS, #marker> }
        })
        .collect();

    let field_inits: Vec<&syn::Ident> = fields.iter().map(|(n, _)| *n).collect();

    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        struct #data_name<__M> {
            #(#field_decls,)*
            __r2e_markers: ::std::marker::PhantomData<fn() -> __M>,
        }

        impl<__R2eS, #(#marker_idents),*> #krate::http::extract::FromRequestParts<__R2eS>
            for #data_name<(#(#marker_idents,)*)>
        where
            __R2eS: Send + Sync,
            #(#via_bounds,)*
        {
            type Rejection = #krate::http::response::Response;

            async fn from_request_parts(
                __parts: &mut #krate::http::header::Parts,
                __state: &__R2eS,
            ) -> Result<Self, Self::Rejection> {
                #(#extractions)*
                Ok(Self {
                    #(#field_inits,)*
                    __r2e_markers: ::std::marker::PhantomData,
                })
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

/// Generate `impl ContextConstruct for Name`.
///
/// Always emitted: the physical core holds only app/config-scoped fields
/// (every request-scoped field moved to the façade), so it is buildable from
/// the resolved bean graph by type. `register_controller()` constructs it once
/// from the retained `BeanContext` and shares it between routes, consumers,
/// and scheduled tasks.
///
/// `Deps` lists the injected bean types (plus `R2eConfig` when config fields
/// are present) so a missing bean is a compile error at the registration call
/// site.
fn generate_context_construct(def: &ControllerStructDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.name;

    let inject_inits: Vec<TokenStream> = def
        .injected_fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_ty = &f.ty;
            quote! { #field_name: __ctx.get::<#field_ty>() }
        })
        .collect();

    let controller_name_str = name.to_string();
    let config_inits: Vec<TokenStream> = def
        .config_fields
        .iter()
        .map(|f| config_init_panic(&f.name, &f.key, &controller_name_str, f.is_option, &krate))
        .collect();

    let config_section_inits: Vec<TokenStream> = def
        .config_section_fields
        .iter()
        .map(|f| config_section_init_panic(&f.name, &f.ty, &f.prefix, &controller_name_str, &krate))
        .collect();

    let has_any_config = !def.config_fields.is_empty() || !def.config_section_fields.is_empty();
    let config_prelude = if has_any_config {
        quote! {
            let __cfg = __ctx.get::<#krate::R2eConfig>();
        }
    } else {
        quote! {}
    };

    let all_inits: Vec<&TokenStream> = inject_inits
        .iter()
        .chain(config_inits.iter())
        .chain(config_section_inits.iter())
        .collect();

    // The physical core always has the hidden `__r2e_decos` slot (added by
    // `add_deco_slot_field`), so construction is always a braced literal —
    // including for source-level unit structs.
    let struct_init = quote! {
        Self {
            #(#all_inits,)*
            __r2e_decos: ::core::default::Default::default(),
        }
    };

    // Deps: unique injected field types, plus R2eConfig when config fields
    // exist. Duplicates are excluded — a duplicate requirement is harmless for
    // the check but noisy in errors.
    let mut deps_seen = std::collections::HashSet::new();
    let mut deps_types: Vec<TokenStream> = Vec::new();
    for f in &def.injected_fields {
        let ty = &f.ty;
        let key = quote!(#ty).to_string().replace(' ', "");
        if deps_seen.insert(key) {
            deps_types.push(quote! { #ty });
        }
    }
    if has_any_config {
        deps_types.push(quote! { #krate::R2eConfig });
    }
    let deps_list = crate::type_list_gen::build_tcons_type(&deps_types, &krate);

    quote! {
        impl #krate::ContextConstruct for #name {
            type Deps = #deps_list;

            fn from_context(__ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                #struct_init
            }
        }
    }
}
