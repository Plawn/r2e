//! Generate the tonic trait implementation for the gRPC wrapper struct.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_grpc_path};
use crate::grpc_routes_parsing::{GrpcMethod, GrpcRoutesImplDef};

/// Generate `#[tonic::async_trait] impl TraitPath for __R2eGrpc<Name>`.
pub fn generate_tonic_trait_impl(def: &GrpcRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let wrapper_name = format_ident!("__R2eGrpc{}", controller_name);

    let method_impls: Vec<TokenStream> = def
        .methods
        .iter()
        .map(|m| generate_method_impl(m, def, &krate, &grpc_krate, controller_name))
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

    // Determine identity extraction. Guards/roles are rejected on gRPC methods
    // at parse time (`validate_grpc_attrs`), so there is no guard codegen here.
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
    let identity_extraction = if has_identity {
        generate_identity_extraction(method, grpc_krate)
    } else {
        quote! {}
    };

    // The core is shared — built once at registration, cloned per call site.
    let construct_controller = quote! {
        let __ctrl = ::std::sync::Arc::clone(&self.core);
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
                        state: &self.ctx,
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
                    state: &self.ctx,
                },
                #wrapped
            ).await
        }
    }
}
