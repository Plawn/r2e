//! Generate `impl GrpcService<T>` for the controller struct.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_grpc_path};
use crate::grpc_routes_parsing::GrpcRoutesImplDef;

use super::GrpcDecoSets;

/// Generate the `EndpointDeps` carrier for the service: the core's
/// `ContextConstruct::Deps` extended with every `#[intercept(...)]` site's
/// spec deps — the same fold `#[routes]` emits for HTTP controllers. Checked
/// by `AllSatisfied` at `register_grpc_service()`, so a missing bean is a
/// compile error at the registration call site.
pub fn generate_endpoint_deps_impl(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;

    let mut exprs: Vec<&syn::Expr> = Vec::new();
    // Controller-level interceptors run on every method; their deps only
    // matter when at least one method exists.
    if !def.methods.is_empty() {
        exprs.extend(&def.controller_intercepts);
    }
    for m in &def.methods {
        exprs.extend(&m.decorators.intercept_fns);
    }
    let deps_fold = crate::codegen::decorators::endpoint_deps_fold(controller_name, exprs);

    quote! {
        #[doc(hidden)]
        impl #krate::EndpointDeps for #controller_name {
            type Deps = #deps_fold;
        }
    }
}

/// Generate `impl GrpcService for ControllerName`.
pub fn generate_grpc_service_impl(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let wrapper_name = format_ident!("__R2eGrpc{}", controller_name);

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

    // Prebuild every method's interceptor set from the resolved graph — once,
    // at registration, exactly like route decorator sets — into the single
    // Arc'd container.
    let decos_init = if deco.has_any() {
        let container = GrpcDecoSets::container_ident(controller_name);
        let field_inits: Vec<TokenStream> = deco
            .fields(def)
            .map(|(field, set)| {
                let ctor = &set.ctor_ident;
                quote! { #field: #ctor(__ctx) }
            })
            .collect();
        quote! {
            __decos: ::std::sync::Arc::new(#container {
                #(#field_inits,)*
            }),
        }
    } else {
        quote! {}
    };

    // Override the trait's `None` default only when the attribute declared a
    // descriptor set (`#[grpc_routes(..., descriptor = <expr>)]`).
    let descriptor_impl = def.descriptor.as_ref().map(|expr| {
        quote! {
            fn file_descriptor_set() -> Option<&'static [u8]> {
                Some(#expr)
            }
        }
    });

    quote! {
        impl #grpc_krate::GrpcService for #controller_name {
            fn service_name() -> &'static str {
                #service_name
            }

            #descriptor_impl

            fn add_to_routes(
                __routes: #grpc_krate::tonic::service::Routes,
                __ctx: &::std::sync::Arc<#krate::beans::BeanContext>,
            ) -> #grpc_krate::tonic::service::Routes {
                let wrapper = #wrapper_name {
                    core: ::std::sync::Arc::new(
                        <#controller_name as #krate::ContextConstruct>::from_context(__ctx),
                    ),
                    #decos_init
                };
                __routes.add_service(#server_path::new(wrapper))
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
