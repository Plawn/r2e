//! Generate `impl GrpcService<T>` for the controller struct.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_grpc_path};
use crate::grpc_routes_parsing::GrpcRoutesImplDef;

/// Generate `impl GrpcService<State> for ControllerName`.
pub fn generate_grpc_service_impl(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let wrapper_name = format_ident!("__R2eGrpc_{}", controller_name);
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);

    // Derive the server type path from the trait path.
    // Convention: if trait is `proto::user_service_server::UserService`,
    // the server is `proto::user_service_server::UserServiceServer`.
    let server_path = derive_server_path(service_trait);

    // The service name is derived from the trait name.
    let service_name = service_trait
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();

    quote! {
        impl #grpc_krate::GrpcService<#meta_mod::State> for #controller_name {
            fn service_name() -> &'static str {
                #service_name
            }

            fn into_router(state: &#meta_mod::State) -> #grpc_krate::tonic::transport::server::Router {
                let wrapper = #wrapper_name { state: state.clone() };
                #grpc_krate::tonic::transport::Server::builder()
                    .add_service(#server_path::new(wrapper))
            }
        }
    }
}

/// Derive the `*Server` path from the trait path.
///
/// For example:
/// - `proto::user_service_server::UserService` → `proto::user_service_server::UserServiceServer`
/// - `my_proto::GreeterService` → `my_proto::GreeterServiceServer`
fn derive_server_path(trait_path: &syn::Path) -> syn::Path {
    let mut server_path = trait_path.clone();
    if let Some(last_segment) = server_path.segments.last_mut() {
        let server_name = format!("{}Server", last_segment.ident);
        last_segment.ident = syn::Ident::new(&server_name, last_segment.ident.span());
        // Clear any generic arguments
        last_segment.arguments = syn::PathArguments::None;
    }
    server_path
}
