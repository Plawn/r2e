//! Method wrapping for transactional behavior.
//!
//! Interceptor wrapping has moved to `handlers.rs` where it has access
//! to the application state (needed by `InterceptorContext<'_, S>`).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

/// Generate the impl block with wrapped methods.
pub fn generate_impl_block(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;

    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(|rm| generate_wrapped_method(rm))
        .collect();

    let consumer_fns: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let f = &cm.fn_item;
            quote! { #f }
        })
        .collect();

    let sse_fns: Vec<TokenStream> = def
        .sse_methods
        .iter()
        .map(|sm| {
            let f = &sm.fn_item;
            quote! { #f }
        })
        .collect();

    let ws_fns: Vec<TokenStream> = def
        .ws_methods
        .iter()
        .map(|wm| {
            let f = &wm.fn_item;
            quote! { #f }
        })
        .collect();

    let scheduled_fns: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| generate_wrapped_scheduled_method(sm, def))
        .collect();

    let other_fns: Vec<_> = def.other_methods.iter().collect();

    if route_fns.is_empty()
        && sse_fns.is_empty()
        && ws_fns.is_empty()
        && consumer_fns.is_empty()
        && scheduled_fns.is_empty()
        && other_fns.is_empty()
    {
        quote! {}
    } else {
        quote! {
            impl #name {
                #(#route_fns)*
                #(#sse_fns)*
                #(#ws_fns)*
                #(#consumer_fns)*
                #(#scheduled_fns)*
                #(#other_fns)*
            }
        }
    }
}

/// Wrap a route method with transactional behavior only.
/// Interceptors are now handled at the handler level (handlers.rs).
fn generate_wrapped_method(rm: &RouteMethod) -> TokenStream {
    if rm.decorators.transactional.is_none() {
        let f = &rm.fn_item;
        return quote! { #f };
    }

    let krate = r2e_core_path();
    let fn_item = &rm.fn_item;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let sig = &fn_item.sig;
    let original_body = &fn_item.block;

    let mut body: TokenStream = quote! { #original_body };

    // Inline wrapper: transactional
    if let Some(ref tx_config) = rm.decorators.transactional {
        let pool_field = format_ident!("{}", tx_config.pool_field);
        body = quote! {
            {
                let mut tx = self.#pool_field.begin().await
                    .map_err(|__e| #krate::HttpError::Internal(__e.to_string()))?;
                let __tx_result = #body;
                match __tx_result {
                    Ok(__val) => {
                        tx.commit().await
                            .map_err(|__e| #krate::HttpError::Internal(__e.to_string()))?;
                        Ok(__val)
                    }
                    Err(__err) => Err(__err),
                }
            }
        };
    }

    quote! {
        #(#attrs)*
        #vis #sig {
            #body
        }
    }
}

/// Wrap a scheduled method with interceptors.
/// Scheduled methods keep interceptor wrapping here because they don't
/// go through the handler path (no State extraction).
fn generate_wrapped_scheduled_method(sm: &ScheduledMethod, def: &RoutesImplDef) -> TokenStream {
    let has_interceptors = !sm.intercept_fns.is_empty() || !def.controller_intercepts.is_empty();

    if !has_interceptors {
        let f = &sm.fn_item;
        return quote! { #f };
    }

    let fn_item = &sm.fn_item;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let sig = &fn_item.sig;
    let fn_name_str = sig.ident.to_string();
    let controller_name_str = def.controller_name.to_string();
    let original_body = &fn_item.block;

    let body = wrap_with_interceptors_no_state(
        quote! { #original_body },
        &fn_name_str,
        &controller_name_str,
        def,
        &sm.intercept_fns,
    );

    quote! {
        #(#attrs)*
        #vis #sig {
            #body
        }
    }
}

/// Apply interceptor chain without state (for scheduled tasks).
/// Uses a unit-type `()` state since scheduled tasks don't have HTTP state.
fn wrap_with_interceptors_no_state(
    mut body: TokenStream,
    fn_name_str: &str,
    controller_name_str: &str,
    def: &RoutesImplDef,
    method_intercepts: &[syn::Expr],
) -> TokenStream {
    let all_intercepts: Vec<&syn::Expr> = def
        .controller_intercepts
        .iter()
        .chain(method_intercepts.iter())
        .collect();

    if all_intercepts.is_empty() {
        return body;
    }

    let krate = r2e_core_path();

    for intercept_expr in all_intercepts.iter().rev() {
        body = quote! {
            {
                let __interceptor = #intercept_expr;
                #krate::Interceptor::around(&__interceptor, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    quote! {
        {
            let __ctx = #krate::InterceptorContext {
                method_name: #fn_name_str,
                controller_name: #controller_name_str,
                state: &(),
            };
            #body
        }
    }
}
