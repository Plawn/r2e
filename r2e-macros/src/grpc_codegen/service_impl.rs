//! Generate `impl GrpcService<S, W>` for the controller struct.

use proc_macro2::TokenStream;
use quote::quote;

use crate::parsing::grpc_routes_parsing::GrpcRoutesImplDef;
use crate::util::crate_path::{r2e_core_path, r2e_grpc_path};

use crate::codegen::registration::DepsFold;

use super::{controller_items, service_bounds, service_name_expr, site_exprs, witness, GrpcDecoSets};

/// Generate the `EndpointDeps` carrier for the service: the core's
/// `ContextConstruct::Deps` extended with every guard/interceptor site's
/// spec deps — the same fold `#[routes]` emits for HTTP controllers. Checked
/// by `AllSatisfied` at `register_grpc_service()`, so a missing bean is a
/// compile error at the registration call site. Identity extractors are not
/// folded here: their bean deps (the JWT validator) are `FromRequestPartsVia`
/// bounds on the `GrpcService<S, W>` impl, checked by inferring `W` at the
/// same call.
pub fn generate_endpoint_deps_impl(def: &GrpcRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;
    DepsFold::endpoint(controller_name, site_exprs(def)).carrier_impl(controller_name)
}

/// Generate `impl<S, W> GrpcService<S, W> for ControllerName`.
///
/// Mirrors the `McpService<S, W>` impl `#[routes]` emits for MCP members: generic over
/// the state `__R2eS`, the request-data marker `__R2eMd` (bound
/// `__R2eRequestData_<Name><__R2eMd>: RequestData<__R2eS>` — the struct's
/// `#[inject(identity)]` / `#[inject(request)]` fields) plus one opaque
/// marker `__R2eMp<Fn>` per method that declares an `#[inject(identity)]`
/// parameter, bound `<declared ty>: FromRequestPartsVia<__R2eS, __R2eMp<Fn>>`.
/// The markers are folded into the witness `W`, so
/// `register_grpc_service::<Svc>()` infers them — and fails to compile when
/// the state cannot satisfy an extractor (e.g. no `Arc<JwtClaimsValidator>`
/// bean behind `AuthenticatedUser`).
pub fn generate_grpc_service_impl(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let items = controller_items(controller_name);
    let wrapper_name = &items.wrapper_name;
    let meta_mod = &items.meta_mod;

    // Derive the server type path from the trait path.
    // Convention: if trait is `proto::user_service_server::UserService`,
    // the server is `proto::user_service_server::UserServiceServer`.
    let server_path = derive_server_path(service_trait);
    let service_name = service_name_expr(service_trait);

    let witness = witness(def);
    let witness_generics = witness.generics();
    let witness_tuple = witness.tuple();
    let bounds = service_bounds(def, &witness);

    // Prebuild every method's guard/interceptor set from the resolved graph —
    // once, at registration, exactly like route decorator sets — into the
    // single Arc'd container.
    let decos_init = if deco.has_any() {
        let container = GrpcDecoSets::container_ident(controller_name);
        let field_inits = deco.sets.field_inits();
        quote! {
            __decos: ::std::sync::Arc::new(#container {
                #(#field_inits,)*
            }),
        }
    } else {
        quote! {}
    };

    // Aggregated config validation for the service: the core's own
    // `#[config]`/`#[config_section]` keys (from the `#[controller]`-generated
    // meta module) plus every guard/interceptor spec's declared keys.
    // Reported at `register_grpc_service()`, the gRPC peer of
    // `register_controller()`.
    let decorator_config_stmts =
        crate::codegen::decorators::decorator_config_key_stmts(site_exprs(def));

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
        impl<__R2eS, #witness_generics> #grpc_krate::GrpcService<__R2eS, #witness_tuple>
            for #controller_name
        where
            #bounds
        {
            fn service_name() -> &'static str {
                #service_name
            }

            #descriptor_impl

            fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                #[allow(unused_mut)]
                let mut __errors = #meta_mod::validate_config(__config);
                #(#decorator_config_stmts)*
                __errors
            }

            fn add_to_routes(
                __routes: #grpc_krate::tonic::service::Routes,
                __state: &__R2eS,
                __ctx: &::std::sync::Arc<#krate::beans::BeanContext>,
            ) -> #grpc_krate::tonic::service::Routes {
                let wrapper = #wrapper_name::<__R2eS, #witness_tuple> {
                    core: ::std::sync::Arc::new(
                        <#controller_name as #krate::ContextConstruct>::from_context(__ctx),
                    ),
                    state: ::std::sync::Arc::new(::core::clone::Clone::clone(__state)),
                    #decos_init
                    _w: ::core::marker::PhantomData,
                };
                __routes.add_service(#grpc_krate::__macro_support::CaptureMethodPath(
                    #server_path::new(wrapper),
                ))
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
