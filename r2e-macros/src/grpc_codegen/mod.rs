//! Code generation for the `#[grpc_routes]` attribute macro.
//!
//! Generates:
//! - The user's impl block (methods with stripped attributes)
//! - A wrapper struct `__R2eGrpc<Name>` that holds the shared core and the
//!   prebuilt per-method guard/interceptor sets + identity extractors
//! - An impl of the tonic-generated trait for the wrapper
//! - An impl of `GrpcService<T>` for the controller
//!
//! Per call the generated trait method runs: identity extraction
//! (`GrpcIdentity::extract`, when the method has an `#[inject(identity)]`
//! parameter) → controller guards → method guards → interceptor chain →
//! user method. A `Rejection` from identity or a guard is projected onto a
//! `tonic::Status` by kind (`rejection_to_status`) — the gRPC leg of the
//! #1072 error-projection model.

mod service_impl;
mod trait_impl;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::codegen::decorators::{generate_named_deco_items, spec_type_of, DecoSet};
use crate::parsing::grpc_routes_parsing::GrpcRoutesImplDef;
use crate::util::crate_path::r2e_core_path;

/// Per-method prebuilt guard/interceptor sets for a gRPC impl block.
///
/// `sets` is parallel to `def.methods`: `None` when the method has no
/// guard/interceptor sites (or when spec inference failed — the
/// `compile_error!` then lives in `items` and the method degrades to the
/// unguarded, unwrapped shape).
///
/// All sets — and every method's identity extractor — live in one hidden
/// container struct behind a single `Arc` on the wrapper (`__decos`), so
/// cloning the wrapper (tonic clones the service per call) costs one
/// ref-count bump regardless of how many methods are decorated.
pub(crate) struct GrpcDecoSets {
    pub items: TokenStream,
    sets: Vec<Option<DecoSet>>,
}

impl GrpcDecoSets {
    /// The hidden container struct holding every method's prebuilt set.
    pub fn container_ident(controller_name: &syn::Ident) -> syn::Ident {
        format_ident!("__R2eGrpcDecos_{}", controller_name)
    }

    /// The container field holding one method's prebuilt set.
    pub fn field_ident(fn_name: &syn::Ident) -> syn::Ident {
        format_ident!("__deco_{}", fn_name)
    }

    /// The container field holding one method's identity extractor
    /// (`<I as GrpcIdentity>::Spec::Product`).
    pub fn identity_field_ident(fn_name: &syn::Ident) -> syn::Ident {
        format_ident!("__id_{}", fn_name)
    }

    /// Whether the container exists: any method has a prebuilt set or an
    /// identity extractor.
    pub fn has_any(&self, def: &GrpcRoutesImplDef) -> bool {
        self.sets.iter().any(Option::is_some)
            || def.methods.iter().any(|m| m.identity_param.is_some())
    }

    /// The set for one method, positionally paired with `def.methods`.
    pub fn set_for(&self, index: usize) -> Option<&DecoSet> {
        self.sets[index].as_ref()
    }

    /// `(container field, set)` for every decorated method, in
    /// `def.methods` order — the single source of the method ↔ field
    /// pairing shared by the container decl, its init, and the trait impl.
    pub fn fields<'a>(
        &'a self,
        def: &'a GrpcRoutesImplDef,
    ) -> impl Iterator<Item = (syn::Ident, &'a DecoSet)> {
        def.methods
            .iter()
            .zip(self.sets.iter())
            .filter_map(|(m, set)| set.as_ref().map(|s| (Self::field_ident(&m.name), s)))
    }

    /// `(container field, identity type)` for every method with an
    /// `#[inject(identity)]` parameter, in `def.methods` order.
    pub fn identity_fields(
        def: &GrpcRoutesImplDef,
    ) -> impl Iterator<Item = (syn::Ident, &syn::Type)> {
        def.methods.iter().filter_map(|m| {
            m.identity_param
                .as_ref()
                .map(|p| (Self::identity_field_ident(&m.name), &p.ty))
        })
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
        exprs.extend(&m.decorators.guard_fns);
        exprs.extend(&m.decorators.intercept_fns);
    }
    exprs
}

/// The distinct identity types injected by the impl's methods, in first-seen
/// order. Their `GrpcIdentity::Spec` deps join the service's `EndpointDeps`
/// (and config validation) exactly like decorator specs.
pub(crate) fn unique_identity_types(def: &GrpcRoutesImplDef) -> Vec<&syn::Type> {
    let mut seen = std::collections::HashSet::new();
    let mut types = Vec::new();
    for m in &def.methods {
        if let Some(p) = &m.identity_param {
            let ty = &p.ty;
            if seen.insert(quote!(#ty).to_string()) {
                types.push(ty);
            }
        }
    }
    types
}

/// Build the decorator sets (hidden struct + ctor per method) from the guard
/// and interceptor sites. Controller-level sites first, then method-level —
/// same execution order as HTTP routes and MCP members.
fn build_deco_sets(def: &GrpcRoutesImplDef) -> GrpcDecoSets {
    let mut items = quote! {};
    let mut sets = Vec::with_capacity(def.methods.len());
    for method in &def.methods {
        let guard_exprs: Vec<syn::Expr> = def
            .controller_guards
            .iter()
            .chain(method.decorators.guard_fns.iter())
            .cloned()
            .collect();
        let intercept_exprs: Vec<&syn::Expr> = def
            .controller_intercepts
            .iter()
            .chain(method.decorators.intercept_fns.iter())
            .collect();
        let (method_items, set) = generate_named_deco_items(
            &def.controller_name,
            "GrpcDeco",
            &method.name,
            &guard_exprs,
            &intercept_exprs,
            quote! {},
        );
        items.extend(method_items);
        sets.push(set);
    }
    GrpcDecoSets { items, sets }
}

/// Compile-time identity checks:
///
/// - `#[grpc_routes]` has no struct-level identity (gRPC identity is per
///   method, like MCP) — assert the `#[controller]` meta says so.
/// - Per guard site, `DecoratorSpec::REQUIRES_IDENTITY` must be satisfiable:
///   a guard that needs a `Some` identity on a method without an
///   `#[inject(identity)]` parameter can never pass. Non-inferable specs are
///   skipped (their spec-type error already fails the build).
fn generate_identity_asserts(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);
    let mut asserts = vec![quote_spanned! { def.controller_name.span() =>
        const _: () = ::core::assert!(
            !#meta_mod::HAS_STRUCT_IDENTITY,
            "#[grpc_routes] does not support struct-level #[inject(identity)]: gRPC identity is per method; add an `#[inject(identity)]` parameter on the method"
        );
    }];
    for method in &def.methods {
        let cond = method.identity_param.is_some();
        for expr in def
            .controller_guards
            .iter()
            .chain(method.decorators.guard_fns.iter())
        {
            let Ok((spec_ty, _)) = spec_type_of(expr) else {
                continue;
            };
            let span = expr.span();
            asserts.push(quote_spanned! { span =>
                const _: () = ::core::assert!(
                    !<#spec_ty as #krate::DecoratorSpec>::REQUIRES_IDENTITY || #cond,
                    "this #[guard] requires an authenticated identity, but the gRPC method can \
                     never provide one: add an `#[inject(identity)]` parameter on the method"
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
    let deco_items = &deco.items;

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

/// Generate the user's impl block with route attributes stripped.
fn generate_impl_block(def: &GrpcRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;

    let methods: Vec<&syn::ImplItemFn> = def
        .methods
        .iter()
        .map(|m| &m.fn_item)
        .chain(def.other_methods.iter())
        .collect();

    quote! {
        impl #controller_name {
            #(#methods)*
        }
    }
}

/// Generate the wrapper struct that holds the controller core + the prebuilt
/// decorator container, plus the container struct itself.
///
/// The wrapper is what actually implements the tonic trait. The controller
/// core is built ONCE from the bean graph (`ContextConstruct`) when the
/// service is registered; requests share it through the `Arc`. Guard /
/// interceptor sets and identity extractors are built at the same time, from
/// the same context (`DecoratorSpec::build`) — never per call.
fn generate_wrapper_struct(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = crate::util::crate_path::r2e_grpc_path();
    let controller_name = &def.controller_name;
    let wrapper_name = quote::format_ident!("__R2eGrpc{}", controller_name);

    let (container_decl, decos_field) = if deco.has_any(def) {
        let container = GrpcDecoSets::container_ident(controller_name);
        let mut fields: Vec<TokenStream> = deco
            .fields(def)
            .map(|(field, set)| {
                let ty = set.ty();
                quote! { #field: #ty }
            })
            .collect();
        fields.extend(GrpcDecoSets::identity_fields(def).map(|(field, ty)| {
            quote! {
                #field: <<#ty as #grpc_krate::__macro_support::GrpcIdentity>::Spec
                    as #krate::DecoratorSpec>::Product
            }
        }));
        (
            quote! {
                #[allow(non_camel_case_types)]
                #[doc(hidden)]
                struct #container {
                    #(#fields,)*
                }
            },
            quote! { __decos: ::std::sync::Arc<#container>, },
        )
    } else {
        (quote! {}, quote! {})
    };

    quote! {
        #container_decl

        #[doc(hidden)]
        #[derive(Clone)]
        pub struct #wrapper_name {
            core: ::std::sync::Arc<#controller_name>,
            #decos_field
        }
    }
}
