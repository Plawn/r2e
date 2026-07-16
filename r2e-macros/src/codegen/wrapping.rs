//! Method wrapping for transactional behavior.
//!
//! Interceptor wrapping for routes lives in `handlers.rs`, and for scheduled
//! tasks in `controller_impl.rs` (interceptors are prebuilt decorator-set
//! fields in both cases).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::codegen::transverse;
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
///
/// `#[anonymous]` routes are off-request too: they run without identity
/// extraction, so they are emitted on the core — reading the identity (or any
/// request-scoped field) from an anonymous route body fails to compile, same
/// diagnostic as consumers.
pub fn generate_impl_block(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;
    let facade_name = format_ident!("__R2eRequest_{}", name);

    // ── Façade (request-scoped) methods ──
    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .filter(|rm| !rm.decorators.anonymous)
        .map(generate_wrapped_method)
        .collect();

    let sse_fns: Vec<TokenStream> = def
        .sse_methods
        .iter()
        .filter(|sm| !sm.decorators.anonymous)
        .map(|sm| {
            let f = &sm.fn_item;
            quote! { #f }
        })
        .collect();

    let ws_fns: Vec<TokenStream> = def
        .ws_methods
        .iter()
        .filter(|wm| !wm.decorators.anonymous)
        .map(|wm| {
            let f = &wm.fn_item;
            quote! { #f }
        })
        .collect();

    // ── Core (anonymous) route methods ──
    let anon_route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .filter(|rm| rm.decorators.anonymous)
        .map(generate_wrapped_method)
        .collect();

    let anon_sse_ws_fns: Vec<TokenStream> = def
        .sse_methods
        .iter()
        .filter(|sm| sm.decorators.anonymous)
        .map(|sm| &sm.fn_item)
        .chain(
            def.ws_methods
                .iter()
                .filter(|wm| wm.decorators.anonymous)
                .map(|wm| &wm.fn_item),
        )
        .map(|f| quote! { #f })
        .collect();

    // ── Core (off-request) methods ──
    //
    // `#[intercept(...)]` sets are built once from the bean context at
    // registration (`fill_decos` → `BeanDecoFill`) and stored in the core's
    // hidden `DecoSlot`. Intercepted scheduled/consumer methods run the chain
    // in their own dispatch wrapper (slot lookup), so DIRECT in-code calls are
    // intercepted too — not just scheduler ticks / event delivery. A sync
    // scheduled source's wrapper is PROMOTED to `async fn` so the body can
    // await the chain (DI backlog item 11).
    let consumer_fns: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let intercept_exprs: Vec<&syn::Expr> = def
                .controller_intercepts
                .iter()
                .chain(cm.intercept_fns.iter())
                .collect();
            let event_param = cm.fn_item.sig.inputs.iter().find_map(|arg| match arg {
                syn::FnArg::Typed(pt) => Some(pt.clone()),
                _ => None,
            });
            generate_transverse_method(
                &cm.fn_item,
                &intercept_exprs,
                true, // consumers are always async
                event_param,
                format_ident!("__r2e_cons_{}_inner", cm.fn_item.sig.ident),
                def,
            )
        })
        .collect();

    let scheduled_fns: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| {
            let intercept_exprs: Vec<&syn::Expr> = def
                .controller_intercepts
                .iter()
                .chain(sm.intercept_fns.iter())
                .collect();
            generate_transverse_method(
                &sm.fn_item,
                &intercept_exprs,
                sm.fn_item.sig.asyncness.is_some(),
                None, // scheduled methods take only &self
                format_ident!("__r2e_sched_{}_inner", sm.fn_item.sig.ident),
                def,
            )
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
        && anon_route_fns.is_empty()
        && anon_sse_ws_fns.is_empty()
    {
        quote! {}
    } else {
        quote! {
            impl #name {
                #(#anon_route_fns)*
                #(#anon_sse_ws_fns)*
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

/// Emit one off-request (scheduled / consumer) method, wrapping it in the
/// interceptor dispatch shell when it has (inferable) `#[intercept]` sites.
///
/// The wrapper reads the prebuilt set from the core's `DecoSlot` (filled at
/// registration by `fill_decos` → `BeanDecoFill`) and runs the chain around the
/// inner call, so the chain applies on every call path. An unregistered core
/// (slot empty — e.g. a hand-built test core) runs the inner body undecorated.
///
/// A **sync** scheduled source's wrapper is PROMOTED to `async fn` (consumers
/// are already async) — the chain can only run in an async body. The inner fn
/// keeps the source signature; the promotion is flagged in the wrapper's
/// rustdoc.
///
/// Methods without interceptors and methods whose spec type is not inferable
/// (the compile_error is emitted by the registration pass) are emitted
/// unchanged. Shared with the consumer path via
/// [`transverse::intercepted_dispatch_wrapper`].
fn generate_transverse_method(
    fn_item: &syn::ImplItemFn,
    intercept_exprs: &[&syn::Expr],
    source_async: bool,
    event_param: Option<syn::PatType>,
    inner_name: syn::Ident,
    def: &RoutesImplDef,
) -> TokenStream {
    if intercept_exprs.is_empty()
        || !super::decorators::all_specs_inferable(intercept_exprs.iter().copied())
    {
        return quote! { #fn_item };
    }

    let params = transverse::DispatchWrapperParams {
        container: super::decorators::sched_container_ident(&def.controller_name),
        field: super::decorators::sched_field_ident(&fn_item.sig.ident),
        // Controller cores read their `DecoSlot` field directly (not through
        // `HasDecoSlot` — that is the bean's `SharedDecoSlot`).
        slot_access: quote! { self.__r2e_decos },
        inner_name,
        owner_name_str: def.controller_name.to_string(),
        source_async,
        event_param,
        intercept_count: intercept_exprs.len(),
        origin_macro: "#[routes]",
    };
    // Controller cores are built via `ContextConstruct`, not struct literals in
    // this impl — no slot-field injection, so the block transform is a no-op.
    transverse::intercepted_dispatch_wrapper(fn_item, &params, |_block| {})
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

