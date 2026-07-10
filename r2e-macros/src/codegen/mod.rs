//! Code generation for the `#[routes]` attribute macro.
//!
//! This module is organized into four submodules:
//!
//! - `wrapping`: Method wrapping with interceptors and transactional behavior
//! - `handlers`: Axum handler function generation
//! - `decorators`: Guard/interceptor decorator sets (built once from the graph)
//! - `controller_impl`: Controller trait implementation generation

pub(crate) mod controller_impl;
pub(crate) mod decorators;
mod handlers;
mod wrapping;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};

use crate::routes_parsing::RoutesImplDef;
use crate::types::MethodDecorators;

/// Main entry point: generate all code for a `#[routes]` impl block.
pub fn generate(def: &RoutesImplDef) -> TokenStream {
    let impl_block = wrapping::generate_impl_block(def);
    let handlers = handlers::generate_handlers(def);
    let controller_impl = controller_impl::generate_controller_impl(def);
    let anonymous_asserts = generate_anonymous_asserts(def);

    quote! {
        #impl_block
        #handlers
        #controller_impl
        #(#anonymous_asserts)*
    }
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
