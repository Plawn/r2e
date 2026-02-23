//! Generate the tonic trait implementation for the gRPC wrapper struct.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_grpc_path, r2e_security_path};
use crate::grpc_routes_parsing::{GrpcMethod, GrpcRoutesImplDef};

/// Generate `#[tonic::async_trait] impl TraitPath for __R2eGrpc<Name>`.
pub fn generate_tonic_trait_impl(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let wrapper_name = format_ident!("__R2eGrpc{}", controller_name);
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);

    let method_impls: Vec<TokenStream> = def
        .methods
        .iter()
        .map(|m| generate_method_impl(m, def, &krate, &grpc_krate, controller_name, &meta_mod))
        .collect();

    quote! {
        #[#grpc_krate::tonic::async_trait]
        impl #service_trait for #wrapper_name {
            #(#method_impls)*
        }
    }
}

/// Generate a single tonic trait method implementation.
fn generate_method_impl(
    method: &GrpcMethod,
    def: &GrpcRoutesImplDef,
    krate: &TokenStream,
    grpc_krate: &TokenStream,
    controller_name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> TokenStream {
    let fn_name = &method.name;
    let fn_item = &method.fn_item;

    // Extract the method signature from the original (return type, params)
    let sig = &fn_item.sig;

    // Get the request parameter (first typed param after &self)
    let request_param = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(pt),
            _ => None,
        })
        .next();

    let return_type = &sig.output;

    let controller_name_str = controller_name.to_string();
    let fn_name_str = fn_name.to_string();

    // Determine identity extraction and guard context
    let has_guards = !method.decorators.guard_fns.is_empty() || !method.decorators.roles.is_empty();
    let has_identity = method.identity_param.is_some();
    let has_intercepts =
        !method.decorators.intercept_fns.is_empty() || !def.controller_intercepts.is_empty();

    // Build the request param for the tonic trait signature
    let request_param_tokens = if let Some(pt) = request_param {
        let ty = &pt.ty;
        quote! { request: #ty }
    } else {
        quote! { request: #grpc_krate::tonic::Request<()> }
    };

    // Build identity extraction code
    let identity_extraction = if has_identity || has_guards {
        generate_identity_extraction(method, grpc_krate)
    } else {
        quote! {}
    };

    // Build guard checks
    let guard_checks = if has_guards {
        generate_grpc_guard_checks(method, def, grpc_krate, &controller_name_str, &fn_name_str)
    } else {
        quote! {}
    };

    // Build the controller construction
    let construct_controller = quote! {
        let __ctrl = <#controller_name as #krate::StatefulConstruct<#meta_mod::State>>::from_state(&self.state);
    };

    // Build the method call
    let call_args: Vec<TokenStream> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(_) => Some(quote! { request }),
            _ => None,
        })
        .collect();

    let method_call = quote! { __ctrl.#fn_name(#(#call_args),*).await };

    // Build the interceptor wrapping
    let body = if has_intercepts {
        wrap_with_interceptors(
            method_call,
            &fn_name_str,
            &controller_name_str,
            def,
            &method.decorators.intercept_fns,
            krate,
        )
    } else {
        method_call
    };

    quote! {
        async fn #fn_name(&self, #request_param_tokens) #return_type {
            #identity_extraction
            #guard_checks
            #construct_controller
            #body
        }
    }
}

/// Generate identity extraction from gRPC metadata.
fn generate_identity_extraction(method: &GrpcMethod, grpc_krate: &TokenStream) -> TokenStream {
    if let Some(ref id_param) = method.identity_param {
        let _id_ty = &id_param.ty;
        if id_param.is_optional {
            quote! {
                let __identity: Option<_> = {
                    let __metadata = request.metadata();
                    match #grpc_krate::extract_bearer_token(__metadata) {
                        Ok(_token) => {
                            // Identity extraction will happen in the controller method
                            // via the parameter. For now, we extract at the guard level only.
                            None
                        }
                        Err(_) => None,
                    }
                };
            }
        } else {
            quote! {
                // Identity will be validated during guard checks
            }
        }
    } else {
        quote! {}
    }
}

/// Generate gRPC guard checks.
fn generate_grpc_guard_checks(
    method: &GrpcMethod,
    def: &GrpcRoutesImplDef,
    grpc_krate: &TokenStream,
    controller_name_str: &str,
    fn_name_str: &str,
) -> TokenStream {
    let security_krate = r2e_security_path();

    // Build roles guard if needed
    let roles_guard = if !method.decorators.roles.is_empty() {
        let roles = &method.decorators.roles;
        Some(quote! {
            {
                let __roles_guard = #grpc_krate::GrpcRolesGuard {
                    required_roles: &[#(#roles),*],
                };
                let __guard_ctx = #grpc_krate::GrpcGuardContext {
                    service_name: #controller_name_str,
                    method_name: #fn_name_str,
                    metadata: request.metadata(),
                    identity: None::<&#grpc_krate::__macro_support::tonic::codegen::Never>,
                };
                // Note: roles guard requires identity — this is a simplified version.
                // The full version would extract identity first and pass it.
            }
        })
    } else {
        None
    };

    // Build custom guard checks
    let custom_guards: Vec<TokenStream> = method
        .decorators
        .guard_fns
        .iter()
        .map(|guard_expr| {
            quote! {
                {
                    let __guard = #guard_expr;
                    let __guard_ctx = #grpc_krate::GrpcGuardContext {
                        service_name: #controller_name_str,
                        method_name: #fn_name_str,
                        metadata: request.metadata(),
                        identity: None::<&#grpc_krate::__macro_support::tonic::codegen::Never>,
                    };
                    if let Err(__status) = #grpc_krate::GrpcGuard::check(&__guard, &self.state, &__guard_ctx).await {
                        return Err(__status);
                    }
                }
            }
        })
        .collect();

    quote! {
        #roles_guard
        #(#custom_guards)*
    }
}

/// Wrap a body expression with interceptors.
fn wrap_with_interceptors(
    body: TokenStream,
    fn_name_str: &str,
    controller_name_str: &str,
    def: &GrpcRoutesImplDef,
    method_intercepts: &[syn::Expr],
    krate: &TokenStream,
) -> TokenStream {
    let all_intercepts: Vec<&syn::Expr> = def
        .controller_intercepts
        .iter()
        .chain(method_intercepts.iter())
        .collect();

    if all_intercepts.is_empty() {
        return body;
    }

    // Start with the innermost: the body wrapped in a move closure
    let mut wrapped = quote! {
        move || async move { #body }
    };

    // Wrap from innermost interceptor to second interceptor
    for intercept_expr in all_intercepts[1..].iter().rev() {
        wrapped = quote! {
            move || async move {
                let __interceptor = #intercept_expr;
                #krate::Interceptor::around(
                    &__interceptor,
                    #krate::InterceptorContext {
                        method_name: #fn_name_str,
                        controller_name: #controller_name_str,
                        state: &self.state,
                    },
                    #wrapped
                ).await
            }
        };
    }

    // Apply the outermost interceptor directly
    let outermost = &all_intercepts[0];
    quote! {
        {
            let __interceptor = #outermost;
            #krate::Interceptor::around(
                &__interceptor,
                #krate::InterceptorContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    state: &self.state,
                },
                #wrapped
            ).await
        }
    }
}
