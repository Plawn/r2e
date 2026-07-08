//! Method wrapping for transactional behavior.
//!
//! Interceptor wrapping for routes lives in `handlers.rs`, and for scheduled
//! tasks in `controller_impl.rs` (interceptors are prebuilt decorator-set
//! fields in both cases).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_executor_path};
use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

/// Generate the impl blocks with wrapped methods, split by execution scope.
///
/// Request-scoped methods — HTTP/SSE/WS routes and their generated
/// interceptor/transaction wrappers — are emitted on the request façade
/// `impl __R2eRequest_<Name>`. There `self.<identity/request field>` resolves to
/// a façade field and `self.<injected/config field>` / core helpers resolve
/// through `Deref<Target = Core>`.
///
/// Off-request methods — consumers, `#[scheduled]`, `#[async_exec]`, and
/// ordinary helpers — stay on the core `impl <Name>`. A consumer/scheduled
/// method that touches a request-scoped field therefore fails to compile (the
/// field is not on the core), which is the intended diagnostic.
pub fn generate_impl_block(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;
    let facade_name = format_ident!("__R2eRequest_{}", name);

    // ── Façade (request-scoped) methods ──
    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(generate_wrapped_method)
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

    // ── Core (off-request) methods ──
    let consumer_fns: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let f = &cm.fn_item;
            quote! { #f }
        })
        .collect();

    // Scheduled methods are emitted as-is: their `#[intercept(...)]` sites are
    // built once from the bean context inside `scheduled_tasks_boxed`
    // (controller_impl.rs) and wrap the task invocation there, exactly like
    // route decorators wrap the handler call.
    let scheduled_fns: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| {
            let f = &sm.fn_item;
            quote! { #f }
        })
        .collect();

    let async_exec_fns: Vec<TokenStream> = def
        .async_exec_methods
        .iter()
        .map(generate_async_exec_method)
        .collect();

    let other_fns: Vec<_> = def.other_methods.iter().collect();

    let facade_impl = if route_fns.is_empty() && sse_fns.is_empty() && ws_fns.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #facade_name {
                #(#route_fns)*
                #(#sse_fns)*
                #(#ws_fns)*
            }
        }
    };

    let core_impl = if consumer_fns.is_empty()
        && scheduled_fns.is_empty()
        && async_exec_fns.is_empty()
        && other_fns.is_empty()
    {
        quote! {}
    } else {
        quote! {
            impl #name {
                #(#consumer_fns)*
                #(#scheduled_fns)*
                #(#async_exec_fns)*
                #(#other_fns)*
            }
        }
    };

    quote! {
        #facade_impl
        #core_impl
    }
}

/// Generate the inner async fn (renamed) and a synchronous wrapper that
/// submits the body to the executor and returns `Result<JobHandle<T>, RejectedError>`.
fn generate_async_exec_method(am: &AsyncExecMethod) -> TokenStream {
    let exec_krate = r2e_executor_path();
    let fn_item = &am.fn_item;
    let original_sig = &fn_item.sig;
    let original_name = &original_sig.ident;
    let inner_name = format_ident!("__r2e_async_{}_inner", original_name);
    let executor_field = &am.executor_field;

    let mut inner_fn = fn_item.clone();
    inner_fn.sig.ident = inner_name.clone();

    let return_ty: TokenStream = match &original_sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, ty) => quote! { #ty },
    };

    let typed_inputs: Vec<&syn::PatType> = original_sig
        .inputs
        .iter()
        .filter_map(|a| {
            if let syn::FnArg::Typed(pt) = a {
                Some(pt)
            } else {
                None
            }
        })
        .collect();
    let arg_idents: Vec<syn::Ident> = typed_inputs
        .iter()
        .enumerate()
        .map(|(i, pt)| match &*pt.pat {
            syn::Pat::Ident(pi) => pi.ident.clone(),
            _ => format_ident!("__arg_{}", i),
        })
        .collect();

    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let generics = &original_sig.generics;
    let where_clause = &original_sig.generics.where_clause;

    quote! {
        #inner_fn

        #(#attrs)*
        #vis fn #original_name #generics (
            &self,
            #(#typed_inputs),*
        ) -> ::core::result::Result<#exec_krate::JobHandle<#return_ty>, #exec_krate::RejectedError> #where_clause {
            let __self = ::core::clone::Clone::clone(self);
            self.#executor_field.submit(async move {
                __self.#inner_name(#(#arg_idents),*).await
            })
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
                    .map_err(|__e| #krate::HttpError::Internal(__e.to_string().into()))?;
                let __tx_result = #body;
                match __tx_result {
                    Ok(__val) => {
                        tx.commit().await
                            .map_err(|__e| #krate::HttpError::Internal(__e.to_string().into()))?;
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

