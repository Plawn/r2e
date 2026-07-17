//! Impl-block splitting: façade (request-scoped) vs core (off-request)
//! methods, plus the off-request dispatch wrappers.
//!
//! Interceptor wrapping for routes lives in `handlers.rs`; the shared
//! off-request emitters (`#[scheduled]`/`#[consumer]` dispatch wrappers,
//! `#[async_exec]`) live in `transverse.rs`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::codegen::transverse;
use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

/// Emit an impl method verbatim (no wrapping). Shared by the route/SSE/WS
/// façade sites and the anonymous-route core sites, which all just re-emit the
/// stripped `fn_item` unchanged.
fn emit_verbatim(f: &syn::ImplItemFn) -> TokenStream {
    quote! { #f }
}

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
        .map(|rm| emit_verbatim(&rm.fn_item))
        .collect();

    let sse_fns: Vec<TokenStream> = def
        .sse_methods
        .iter()
        .filter(|sm| !sm.decorators.anonymous)
        .map(|sm| emit_verbatim(&sm.fn_item))
        .collect();

    let ws_fns: Vec<TokenStream> = def
        .ws_methods
        .iter()
        .filter(|wm| !wm.decorators.anonymous)
        .map(|wm| emit_verbatim(&wm.fn_item))
        .collect();

    // ── Core (anonymous) route methods ──
    let anon_route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .filter(|rm| rm.decorators.anonymous)
        .map(|rm| emit_verbatim(&rm.fn_item))
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
        .map(emit_verbatim)
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
            // METHOD-level interceptors only; controller-level (impl-level) ones
            // are shared via the container's `__ctrl` field (see
            // `generate_transverse_method`).
            let intercept_exprs: Vec<&syn::Expr> = cm.intercept_fns.iter().collect();
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
            // METHOD-level interceptors only; controller-level (impl-level) ones
            // are shared via the container's `__ctrl` field.
            let intercept_exprs: Vec<&syn::Expr> = sm.intercept_fns.iter().collect();
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
    // Method-level interceptors that actually resolve to a spec type.
    let method_ok = !intercept_exprs.is_empty()
        && super::decorators::all_specs_inferable(intercept_exprs.iter().copied());
    // Shared controller-level (impl-level) interceptors, built once and read
    // from the container's `__ctrl` field so this method self-intercepts through
    // the same instance every other transverse method / route observes.
    let ctrl_set = super::decorators::ctrl_deco_set(def);
    let ctrl_field_count = ctrl_set.as_ref().map(|s| s.fields.len()).unwrap_or(0);

    // Nothing to wrap: no method-level and no controller-level interceptors.
    if !method_ok && ctrl_field_count == 0 {
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
        intercept_count: if method_ok { intercept_exprs.len() } else { 0 },
        ctrl_field_count,
        origin_macro: "#[routes]",
    };
    // Controller cores are built via `ContextConstruct`, not struct literals in
    // this impl — no slot-field injection, so the block transform is a no-op.
    transverse::intercepted_dispatch_wrapper(fn_item, &params, |_block| {})
}

/// Generate the inner async fn (renamed) and a synchronous wrapper that
/// submits the body to the executor and returns `Result<JobHandle<T>, RejectedError>`.
/// Controller cores are built via `ContextConstruct`, not struct literals in
/// this impl — no slot-field injection, so the inner-block transform is a no-op.
fn generate_async_exec_method(am: &AsyncExecMethod) -> TokenStream {
    transverse::async_exec_method(&am.fn_item, &am.executor_field, |_block| {})
}

