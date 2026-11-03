//! Code generation for the `#[grpc_routes]` attribute macro.
//!
//! Generates:
//! - The user's methods on the request façade `__R2eRequest_<Name>` (generated
//!   by `#[controller]`), its other helpers on the core
//! - A wrapper struct `__R2eGrpc<Name><S, W>` that holds the shared core, the
//!   application state and the prebuilt guard/interceptor sets
//! - An impl of the tonic-generated trait for the wrapper
//! - An impl of `GrpcService<S, W>` for the controller
//!
//! Per call the generated trait method runs: build the HTTP head from the
//! tonic request (`into_head`) → extract the struct's request scope
//! (`RequestData<S>`: struct-level `#[inject(identity)]` /
//! `#[inject(request)]`) and bind the façade → extract the method's
//! `#[inject(identity)]` parameter, if any (`FromRequestPartsVia<S, M>`) →
//! controller guards (shared set, `"*"` context) → method guards (method
//! context) → rebuild the tonic request (`from_head`) → interceptor chain →
//! user method. A `Rejection` from extraction or a guard is projected onto a
//! `tonic::Status` by kind (`rejection_to_status`) — the gRPC leg of the
//! #1072 error-projection model. The extraction contract is HTTP's: the same
//! extractor type works on a route, an MCP member and a gRPC method.

mod service_impl;
mod trait_impl;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::codegen::decorators::{spec_type_of, MemberDecoSets};
use crate::codegen::registration::Witness;
use crate::parsing::grpc_routes_parsing::{GrpcMethod, GrpcRoutesImplDef};
use crate::util::crate_path::r2e_core_path;

/// The gRPC decorator storage: the shared [`MemberDecoSets`] (controller
/// set + one set per method, paired positionally with `def.methods`), all in
/// one hidden container struct behind a single `Arc` on the wrapper
/// (`__decos`) — tonic clones the service per call, so cloning the wrapper
/// costs ref-count bumps regardless of how many methods are decorated.
pub(crate) struct GrpcDecoSets {
    pub sets: MemberDecoSets,
}

impl GrpcDecoSets {
    /// The hidden container struct holding every method's prebuilt set.
    pub fn container_ident(controller_name: &syn::Ident) -> syn::Ident {
        format_ident!("__R2eGrpcDecos_{}", controller_name)
    }

    /// Whether the container exists: any prebuilt set.
    pub fn has_any(&self) -> bool {
        !self.sets.is_empty()
    }
}

/// The names `#[controller]` generates for the struct, which the gRPC codegen
/// binds against.
pub(crate) struct ControllerItems {
    pub meta_mod: syn::Ident,
    pub data_name: syn::Ident,
    pub data_marker: syn::Ident,
    pub facade_name: syn::Ident,
    pub wrapper_name: syn::Ident,
}

pub(crate) fn controller_items(controller_name: &syn::Ident) -> ControllerItems {
    ControllerItems {
        meta_mod: format_ident!("__r2e_meta_{}", controller_name),
        data_name: format_ident!("__R2eRequestData_{}", controller_name),
        data_marker: crate::codegen::handlers::data_marker(),
        facade_name: format_ident!("__R2eRequest_{}", controller_name),
        wrapper_name: format_ident!("__R2eGrpc{}", controller_name),
    }
}

/// A method's `#[inject(identity)]` parameter type as extracted: `I` or
/// `Option<I>`.
pub(crate) fn identity_declared_ty(method: &GrpcMethod) -> Option<TokenStream> {
    let p = method.member.identity_param.as_ref()?;
    let ty = &p.ty;
    Some(if p.is_optional {
        quote_spanned! {ty.span()=> ::core::option::Option<#ty> }
    } else {
        quote! { #ty }
    })
}

/// The witness of the service: the request-data marker plus one opaque
/// marker per method with an identity parameter.
pub(crate) fn witness(def: &GrpcRoutesImplDef) -> Witness {
    let mut witness = Witness::new();
    for m in &def.methods {
        if let Some(declared_ty) = identity_declared_ty(m) {
            witness.push_identity(&m.member.name, declared_ty);
        }
    }
    witness
}

/// The where-clause predicates shared by the tonic trait impl and the
/// `GrpcService<S, W>` impl.
pub(crate) fn service_bounds(def: &GrpcRoutesImplDef, witness: &Witness) -> TokenStream {
    let krate = r2e_core_path();
    let ControllerItems {
        data_name,
        data_marker,
        ..
    } = controller_items(&def.controller_name);
    let data_marker_pred = witness.data_marker_pred();
    let marker_preds = witness.marker_preds();
    let identity_bounds = witness.identity_bounds();
    quote! {
        __R2eS: ::core::clone::Clone
            + ::core::marker::Send
            + ::core::marker::Sync
            + 'static,
        #data_marker_pred
        #data_name<#data_marker>: #krate::web::extract::RequestData<__R2eS>
            + ::core::marker::Send
            + ::core::marker::Sync
            + 'static,
        #marker_preds
        #identity_bounds
    }
}

/// The fully-qualified proto service name: tonic's generated `SERVICE_NAME`
/// const in the `*_server` module (the trait path's parent), e.g.
/// `"greeter.Greeter"`, so logs, the registry, the reflection listing and
/// the request URI all use one naming convention. A single-segment trait
/// path has no visible parent module; fall back to the bare ident.
pub(crate) fn service_name_expr(service_trait: &syn::Path) -> TokenStream {
    if service_trait.segments.len() >= 2 {
        let leading = &service_trait.leading_colon;
        let module_segments: Vec<_> = service_trait
            .segments
            .iter()
            .take(service_trait.segments.len() - 1)
            .collect();
        quote! { #leading #(#module_segments)::* ::SERVICE_NAME }
    } else {
        let name = service_trait
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        quote! { #name }
    }
}

/// Every decorator site expression of the impl block, in the order the dep
/// fold visits them: controller guards + intercepts (only meaningful when at
/// least one method exists), then each method's guards + intercepts. Shared
/// by the `EndpointDeps` fold and the aggregated config validation.
pub(crate) fn site_exprs(def: &GrpcRoutesImplDef) -> Vec<&syn::Expr> {
    let mut exprs: Vec<&syn::Expr> = Vec::new();
    if !def.methods.is_empty() {
        exprs.extend(&def.controller_guards);
        exprs.extend(&def.controller_intercepts);
    }
    for m in &def.methods {
        exprs.extend(&m.member.decorators.guard_fns);
        exprs.extend(&m.member.decorators.intercept_fns);
    }
    exprs
}

/// Build the decorator sets: one shared controller-level set (impl-level
/// guards + interceptors, built once per service) and one set per method
/// holding only that method's own sites. Execution order: controller sites
/// first, then the method's — same as HTTP routes and MCP members.
fn build_deco_sets(def: &GrpcRoutesImplDef) -> GrpcDecoSets {
    GrpcDecoSets {
        sets: MemberDecoSets::build(
            &def.controller_name,
            "Grpc",
            &def.controller_guards,
            &def.controller_intercepts,
            def.methods.iter().map(|m| &m.member),
        ),
    }
}

/// Compile-time identity checks: per guard site,
/// `DecoratorSpec::REQUIRES_IDENTITY` must be satisfiable — a guard that
/// needs a `Some` identity on a method that has neither an
/// `#[inject(identity)]` parameter nor a struct-level `#[inject(identity)]`
/// field can never pass. Non-inferable specs are skipped (their spec-type
/// error already fails the build).
fn generate_identity_asserts(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);
    let mut asserts = Vec::new();
    for method in &def.methods {
        let cond = if method.member.identity_param.is_some() {
            quote! { true }
        } else {
            quote! { #meta_mod::HAS_STRUCT_IDENTITY }
        };
        for expr in def
            .controller_guards
            .iter()
            .chain(method.member.decorators.guard_fns.iter())
        {
            let Ok((spec_ty, _)) = spec_type_of(expr) else {
                continue;
            };
            let span = expr.span();
            asserts.push(quote_spanned! { span =>
                const _: () = ::core::assert!(
                    !<#spec_ty as #krate::DecoratorSpec>::REQUIRES_IDENTITY || #cond,
                    "this #[guard] requires an authenticated identity, but the gRPC method can \
                     never provide one: add a struct-level `#[inject(identity)]` field on the \
                     #[controller] struct or an `#[inject(identity)]` parameter on the method"
                );
            });
        }
    }
    quote! { #(#asserts)* }
}

/// Main entry point: generate all code for a `#[grpc_routes]` impl block.
pub fn generate(def: &GrpcRoutesImplDef) -> TokenStream {
    let deco = build_deco_sets(def);
    let identity_asserts = generate_identity_asserts(def);
    let impl_block = generate_impl_block(def);
    let wrapper = generate_wrapper_struct(def, &deco);
    let tonic_trait_impl = trait_impl::generate_tonic_trait_impl(def, &deco);
    let grpc_service_impl = service_impl::generate_grpc_service_impl(def, &deco);
    let endpoint_deps_impl = service_impl::generate_endpoint_deps_impl(def);
    let deco_items = &deco.sets.items;

    quote! {
        #identity_asserts
        #impl_block
        #deco_items
        #wrapper
        #tonic_trait_impl
        #grpc_service_impl
        #endpoint_deps_impl
    }
}

/// Emit the user's items: the gRPC methods and `#[request_helper]`s on the
/// request façade (they run per call on it, like HTTP handlers and MCP
/// members), every other item on the controller core.
fn generate_impl_block(def: &GrpcRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;
    let facade_name = controller_items(controller_name).facade_name;

    let facade_methods: Vec<TokenStream> = def
        .methods
        .iter()
        .map(|m| {
            let mut f = m.member.fn_item.clone();
            crate::codegen::precise_capture::add_handler_precise_captures(&mut f.sig);
            quote! { #f }
        })
        .chain(def.facade_helpers.iter().map(|f| quote! { #f }))
        .collect();
    let facade_impl = if facade_methods.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #facade_name {
                #(#facade_methods)*
            }
        }
    };
    let other_items = &def.other_items;
    let core_impl = if other_items.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #controller_name {
                #(#other_items)*
            }
        }
    };

    quote! {
        #facade_impl
        #core_impl
    }
}

/// Generate the wrapper struct that holds the controller core, the state and
/// the prebuilt decorator container, plus the container struct itself.
///
/// The wrapper is what actually implements the tonic trait. The controller
/// core is built ONCE from the bean graph (`ContextConstruct`) when the
/// service is registered; requests share it through the `Arc`, and so the
/// state every call extracts against. Guard / interceptor sets are built at
/// the same time, from the same context (`DecoratorSpec::build`) — never per
/// call. The witness `W` is carried as a phantom so the tonic trait impl can
/// name the markers its extraction bounds need.
fn generate_wrapper_struct(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = controller_items(controller_name).wrapper_name;

    let (container_decl, decos_field, decos_clone) = if deco.has_any() {
        let container = GrpcDecoSets::container_ident(controller_name);
        let fields = deco.sets.field_decls();
        (
            quote! {
                #[allow(non_camel_case_types)]
                #[doc(hidden)]
                struct #container {
                    #(#fields,)*
                }
            },
            quote! { __decos: ::std::sync::Arc<#container>, },
            quote! { __decos: ::std::sync::Arc::clone(&self.__decos), },
        )
    } else {
        (quote! {}, quote! {}, quote! {})
    };

    quote! {
        #container_decl

        #[doc(hidden)]
        pub struct #wrapper_name<__R2eS, __R2eW> {
            core: ::std::sync::Arc<#controller_name>,
            state: ::std::sync::Arc<__R2eS>,
            #decos_field
            _w: ::core::marker::PhantomData<fn() -> __R2eW>,
        }

        impl<__R2eS, __R2eW> ::core::clone::Clone for #wrapper_name<__R2eS, __R2eW> {
            fn clone(&self) -> Self {
                Self {
                    core: ::std::sync::Arc::clone(&self.core),
                    state: ::std::sync::Arc::clone(&self.state),
                    #decos_clone
                    _w: ::core::marker::PhantomData,
                }
            }
        }
    }
}
