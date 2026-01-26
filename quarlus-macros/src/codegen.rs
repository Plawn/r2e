use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parsing::*;

pub fn generate(def: &ControllerDef) -> TokenStream {
    let struct_def = generate_struct(def);
    let impl_block = generate_impl(def);
    let handlers = generate_handlers(def);
    let controller_impl = generate_controller_impl(def);

    quote! {
        #struct_def
        #impl_block
        #handlers
        #controller_impl
    }
}

/// Generate the struct definition from inject + identity fields.
fn generate_struct(def: &ControllerDef) -> TokenStream {
    let name = &def.name;

    let fields: Vec<_> = def
        .injected_fields
        .iter()
        .map(|f| {
            let n = &f.name;
            let t = &f.ty;
            quote! { #n: #t }
        })
        .chain(def.identity_fields.iter().map(|f| {
            let n = &f.name;
            let t = &f.ty;
            quote! { #n: #t }
        }))
        .collect();

    quote! {
        pub struct #name {
            #(#fields),*
        }
    }
}

/// Generate `impl Name { ... }` with all original methods.
/// Transactional route methods get their body wrapped with begin/commit/rollback.
fn generate_impl(def: &ControllerDef) -> TokenStream {
    let name = &def.name;

    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(|rm| {
            if rm.transactional {
                generate_transactional_method(rm)
            } else {
                let f = &rm.fn_item;
                quote! { #f }
            }
        })
        .collect();

    let other_fns: Vec<_> = def.other_methods.iter().collect();

    if route_fns.is_empty() && other_fns.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #name {
                #(#route_fns)*
                #(#other_fns)*
            }
        }
    }
}

/// Rewrite a `#[transactional]` method body to wrap it in begin/commit/rollback.
///
/// The original body is placed inside a block assigned to `__tx_result`.
/// - If `?` propagates an error, the function returns early and `tx` is dropped (auto-rollback).
/// - If the block evaluates to `Ok(val)`, we commit.
/// - If the block evaluates to `Err(e)`, `tx` is dropped (auto-rollback).
fn generate_transactional_method(rm: &RouteMethod) -> TokenStream {
    let fn_item = &rm.fn_item;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let sig = &fn_item.sig;
    let original_body = &fn_item.block;

    quote! {
        #(#attrs)*
        #vis #sig {
            let mut tx = self.pool.begin().await
                .map_err(|__e| quarlus_core::AppError::Internal(__e.to_string()))?;
            let __tx_result = #original_body;
            match __tx_result {
                Ok(__val) => {
                    tx.commit().await
                        .map_err(|__e| quarlus_core::AppError::Internal(__e.to_string()))?;
                    Ok(__val)
                }
                Err(__err) => Err(__err),
            }
        }
    }
}

/// Generate free handler functions for every route method.
fn generate_handlers(def: &ControllerDef) -> TokenStream {
    let handlers: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| generate_single_handler(def, rm))
        .collect();

    quote! { #(#handlers)* }
}

fn generate_single_handler(def: &ControllerDef, rm: &RouteMethod) -> TokenStream {
    let controller_name = &def.name;
    let state_type = &def.state_type;
    let fn_name = &rm.fn_item.sig.ident;
    let handler_name = format_ident!("__quarlus_{}_{}", controller_name, fn_name);
    let return_type = &rm.fn_item.sig.output;

    // Identity parameters for the handler signature
    let identity_params: Vec<_> = def
        .identity_fields
        .iter()
        .map(|f| {
            let n = &f.name;
            let t = &f.ty;
            quote! { #n: #t }
        })
        .collect();

    // Extra method parameters (everything except &self)
    let extra_params: Vec<_> = rm
        .fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => Some(pat_type),
            syn::FnArg::Receiver(_) => None,
        })
        .enumerate()
        .collect();

    let handler_extra_params: Vec<_> = extra_params
        .iter()
        .map(|(i, pt)| {
            let arg_name = format_ident!("__arg_{}", i);
            let ty = &pt.ty;
            quote! { #arg_name: #ty }
        })
        .collect();

    let call_args: Vec<_> = extra_params
        .iter()
        .map(|(i, _)| {
            let arg_name = format_ident!("__arg_{}", i);
            quote! { #arg_name }
        })
        .collect();

    // Controller field initialisers
    let inject_inits: Vec<_> = def
        .injected_fields
        .iter()
        .map(|f| {
            let n = &f.name;
            quote! { #n: state.#n.clone() }
        })
        .collect();

    let identity_inits: Vec<_> = def
        .identity_fields
        .iter()
        .map(|f| {
            let n = &f.name;
            quote! { #n: #n }
        })
        .collect();

    let all_inits: Vec<_> = inject_inits
        .iter()
        .chain(identity_inits.iter())
        .cloned()
        .collect();

    let call_expr = if rm.fn_item.sig.asyncness.is_some() {
        quote! { __ctrl.#fn_name(#(#call_args),*).await }
    } else {
        quote! { __ctrl.#fn_name(#(#call_args),*) }
    };

    if rm.roles.is_empty() {
        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                axum::extract::State(state): axum::extract::State<#state_type>,
                #(#identity_params,)*
                #(#handler_extra_params,)*
            ) #return_type {
                let __ctrl = #controller_name {
                    #(#all_inits,)*
                };
                #call_expr
            }
        }
    } else {
        // Role-guarded handler: returns Response so the guard can short-circuit.
        let role_strs = &rm.roles;
        let identity_name = &def.identity_fields[0].name;

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                axum::extract::State(state): axum::extract::State<#state_type>,
                #(#identity_params,)*
                #(#handler_extra_params,)*
            ) -> axum::response::Response {
                if !#identity_name.has_any_role(&[#(#role_strs),*]) {
                    return axum::response::IntoResponse::into_response(
                        quarlus_core::AppError::Forbidden("Insufficient roles".into()),
                    );
                }
                let __ctrl = #controller_name {
                    #(#all_inits,)*
                };
                axum::response::IntoResponse::into_response(#call_expr)
            }
        }
    }
}

/// Generate `impl Controller<T> for Name`.
fn generate_controller_impl(def: &ControllerDef) -> TokenStream {
    let name = &def.name;
    let state_type = &def.state_type;

    let route_registrations: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, rm.fn_item.sig.ident);
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_axum_method_fn());
            quote! {
                .route(#path, axum::routing::#method_fn(#handler_name))
            }
        })
        .collect();

    quote! {
        impl quarlus_core::Controller<#state_type> for #name {
            fn routes() -> axum::Router<#state_type> {
                axum::Router::new()
                    #(#route_registrations)*
            }
        }
    }
}
