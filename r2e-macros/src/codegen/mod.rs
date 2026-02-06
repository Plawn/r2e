//! Code generation for the `#[routes]` attribute macro.
//!
//! This module is organized into three submodules:
//!
//! - `wrapping`: Method wrapping with interceptors and transactional behavior
//! - `handlers`: Axum handler function generation
//! - `controller_impl`: Controller trait implementation generation

mod controller_impl;
mod handlers;
mod wrapping;

use proc_macro2::TokenStream;
use quote::quote;

use crate::routes_parsing::RoutesImplDef;

/// Main entry point: generate all code for a `#[routes]` impl block.
pub fn generate(def: &RoutesImplDef) -> TokenStream {
    let impl_block = wrapping::generate_impl_block(def);
    let handlers = handlers::generate_handlers(def);
    let controller_impl = controller_impl::generate_controller_impl(def);

    quote! {
        #impl_block
        #handlers
        #controller_impl
    }
}
