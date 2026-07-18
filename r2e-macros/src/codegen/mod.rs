//! Code generation for the `#[routes]` attribute macro.
//!
//! This module is organized into six submodules:
//!
//! - `wrapping`: Impl-block splitting (façade vs core) and off-request wrappers
//! - `handlers`: Axum handler function generation
//! - `decorators`: Guard/interceptor decorator sets (built once from the graph)
//! - `scheduled`: Shared `#[scheduled]` config/name/overlap emitters
//! - `transverse`: Shared bean/controller transverse emitters (scheduled
//!   sources, event subscribers, deco containers, dispatch wrappers,
//!   post-construct)
//! - `controller_impl`: Controller trait implementation generation

pub(crate) mod controller_impl;
pub(crate) mod decorators;
mod handlers;
pub(crate) mod scheduled;
pub(crate) mod transverse;
mod wrapping;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::types::MethodDecorators;

/// Main entry point: generate all code for a `#[routes]` impl block.
pub fn generate(def: &RoutesImplDef) -> TokenStream {
    let impl_block = wrapping::generate_impl_block(def);
    let handlers = handlers::generate_handlers(def);
    let controller_impl = controller_impl::generate_controller_impl(def);
    let anonymous_asserts = generate_anonymous_asserts(def);
    let identity_req_asserts = generate_identity_requirement_asserts(def);

    quote! {
        #impl_block
        #handlers
        #controller_impl
        #(#anonymous_asserts)*
        #(#identity_req_asserts)*
    }
}

/// For every `#[guard(...)]` site whose spec declares
/// `DecoratorSpec::REQUIRES_IDENTITY = true`, assert at compile time that the
/// route can actually supply an identity — rejecting statically-always-`None`
/// placements where the guard could only ever 401 (see `FgaCheck`).
///
/// The requirement is a **type-level** const (`REQUIRES_IDENTITY`) known only
/// at type-check time, while "can this route ever hold an identity" is a mix of
/// macro-known facts (a param-level identity, or `#[anonymous]`) and the
/// `#[controller]`-side `HAS_STRUCT_IDENTITY` const. So the check is a
/// cross-macro const-assert combining both, one per identity-requiring guard
/// site (spanned to the guard expression):
///
/// ```ignore
/// const _: () = assert!(!<Spec>::REQUIRES_IDENTITY || <route can hold identity>);
/// ```
///
/// "Can hold an identity" per the guard's `GuardContext` source (see
/// `handlers::generate_guard_context`):
/// - param-level identity (required or `Option<..>`) → `true` (may be `Some`);
/// - `#[anonymous]` with no identity param → `false` (Case C: always `None`);
/// - otherwise the struct identity drives it → `HAS_STRUCT_IDENTITY`
///   (a required OR `Option<..>` struct identity may be `Some`; no field = always
///   `None`).
///
/// Non-inferable guard expressions (the spec type can't be determined) are
/// skipped here — the per-method deco set already emits the `spec_type_of`
/// compile error for them, so emitting a second error would only cascade
/// (same degrade-to-avoid-cascade stance as the rest of `decorators.rs`).
///
/// `#[roles]`/`#[all_roles]` desugar into `RolesGuard`/`AllRolesGuard` guard
/// sites whose `REQUIRES_IDENTITY` is the default `false`, so this assert is a
/// no-op for them: they are already compile-checked through the stronger
/// `RoleBasedIdentity` bound on their `Guard` impl (which `NoIdentity` fails).
fn generate_identity_requirement_asserts(def: &RoutesImplDef) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);

    // The const-bool token for "this route's guards can see a `Some` identity".
    let can_hold_identity = |has_identity_param: bool, anonymous: bool| -> TokenStream {
        if has_identity_param {
            quote! { true }
        } else if anonymous {
            quote! { false }
        } else {
            quote! { #meta_mod::HAS_STRUCT_IDENTITY }
        }
    };

    let mut asserts = Vec::new();

    let mut emit = |guard_exprs: &[syn::Expr], has_identity_param: bool, anonymous: bool| {
        let cond = can_hold_identity(has_identity_param, anonymous);
        for expr in guard_exprs {
            // Skip non-inferable specs — their spec-type error already fails the
            // build; a second diagnostic here would just cascade.
            let Ok((spec_ty, _)) = decorators::spec_type_of(expr) else {
                continue;
            };
            let span = expr.span();
            asserts.push(quote_spanned! { span =>
                const _: () = ::core::assert!(
                    !<#spec_ty as #krate::DecoratorSpec>::REQUIRES_IDENTITY || #cond,
                    "this #[guard] requires an authenticated identity, but the route can never \
                     provide one: add a struct-level `#[inject(identity)]` field or an identity \
                     parameter on the route. An `#[anonymous]` route needs an `Option<..>` \
                     identity parameter to opt back in."
                );
            });
        }
    };

    for rm in &def.route_methods {
        emit(
            &rm.decorators.guard_fns,
            rm.identity_param.is_some(),
            rm.decorators.anonymous,
        );
    }
    for sm in &def.sse_methods {
        emit(
            &sm.decorators.guard_fns,
            sm.identity_param.is_some(),
            sm.decorators.anonymous,
        );
    }
    for wm in &def.ws_methods {
        emit(
            &wm.decorators.guard_fns,
            wm.identity_param.is_some(),
            wm.decorators.anonymous,
        );
    }

    asserts
}

/// If any method carries `#[anonymous]`, assert at compile time that the
/// controller declares a **required** struct-level identity to opt out of.
///
/// `#[routes]` cannot see the struct, so this is checked through the meta
/// module's `STRUCT_IDENTITY_IS_REQUIRED` const. Without a required identity
/// there is no fail-closed baseline: no identity means the routes are already
/// public, and an `Option<T>` identity never rejects — in both cases the
/// marker would be a silent no-op with placement side effects, so reject it.
/// One assert per controller (spanned to the first anonymous method) — every
/// marker on the controller shares the same root cause.
fn generate_anonymous_asserts(def: &RoutesImplDef) -> Vec<TokenStream> {
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);

    let first_anonymous = |decorators: &MethodDecorators, fn_item: &syn::ImplItemFn| {
        decorators.anonymous.then(|| fn_item.sig.ident.span())
    };

    def.route_methods
        .iter()
        .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item))
        .chain(
            def.sse_methods
                .iter()
                .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item)),
        )
        .chain(
            def.ws_methods
                .iter()
                .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item)),
        )
        .next()
        .map(|span| {
            quote_spanned! { span =>
                const _: () = ::core::assert!(
                    #meta_mod::STRUCT_IDENTITY_IS_REQUIRED,
                    "#[anonymous] needs a required struct-level #[inject(identity)] field to opt out of: with no identity the routes are already public, and an Option<..> identity never rejects — the marker would be a no-op"
                );
            }
        })
        .into_iter()
        .collect()
}
