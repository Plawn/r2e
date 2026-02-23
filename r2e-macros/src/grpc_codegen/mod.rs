//! Code generation for the `#[grpc_routes]` attribute macro.
//!
//! Generates:
//! - The user's impl block (methods with stripped attributes)
//! - A wrapper struct `__R2eGrpc<Name>` that holds the state
//! - An impl of the tonic-generated trait for the wrapper
//! - An impl of `GrpcService<T>` for the controller

mod trait_impl;
mod service_impl;

use proc_macro2::TokenStream;
use quote::quote;

use crate::grpc_routes_parsing::GrpcRoutesImplDef;

/// Main entry point: generate all code for a `#[grpc_routes]` impl block.
pub fn generate(def: &GrpcRoutesImplDef) -> TokenStream {
    let impl_block = generate_impl_block(def);
    let wrapper = generate_wrapper_struct(def);
    let tonic_trait_impl = trait_impl::generate_tonic_trait_impl(def);
    let grpc_service_impl = service_impl::generate_grpc_service_impl(def);

    quote! {
        #impl_block
        #wrapper
        #tonic_trait_impl
        #grpc_service_impl
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

/// Generate the wrapper struct that holds the app state.
///
/// The wrapper is what actually implements the tonic trait. It constructs
/// the controller from state via `StatefulConstruct` for each request.
fn generate_wrapper_struct(def: &GrpcRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = quote::format_ident!("__R2eGrpc{}", controller_name);
    let meta_mod = quote::format_ident!("__r2e_meta_{}", controller_name);

    quote! {
        #[doc(hidden)]
        #[derive(Clone)]
        pub struct #wrapper_name {
            state: #meta_mod::State,
        }
    }
}
