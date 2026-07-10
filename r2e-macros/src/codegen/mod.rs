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

/// For every `#[anonymous]` method, assert at compile time that the controller
/// actually declares a struct-level identity to opt out of.
///
/// `#[routes]` cannot see the struct, so this is checked through the meta
/// module's `HAS_STRUCT_IDENTITY` const. Without an identity the marker is
/// dead weight (the route is already public) while still moving the method to
/// the core — reject it instead of silently accepting a no-op with placement
/// side effects.
fn generate_anonymous_asserts(def: &RoutesImplDef) -> Vec<TokenStream> {
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);

    let assert_for = |decorators: &MethodDecorators, fn_item: &syn::ImplItemFn| {
        decorators.anonymous.then(|| {
            let span = fn_item.sig.ident.span();
            quote_spanned! { span =>
                const _: () = ::core::assert!(
                    #meta_mod::HAS_STRUCT_IDENTITY,
                    "#[anonymous] is redundant: this controller declares no #[inject(identity)] struct field, so its routes are already public"
                );
            }
        })
    };

    def.route_methods
        .iter()
        .filter_map(|m| assert_for(&m.decorators, &m.fn_item))
        .chain(
            def.sse_methods
                .iter()
                .filter_map(|m| assert_for(&m.decorators, &m.fn_item)),
        )
        .chain(
            def.ws_methods
                .iter()
                .filter_map(|m| assert_for(&m.decorators, &m.fn_item)),
        )
        .collect()
}
