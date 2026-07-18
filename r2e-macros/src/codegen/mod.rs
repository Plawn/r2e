//! Code generation for the `#[routes]` attribute macro.
//!
//! This module is organized into six submodules:
//!
//! - `wrapping`: Impl-block splitting (façade vs core) and off-request wrappers
//! - `handlers`: Axum handler function generation
//! - `decorators`: Guard/interceptor decorator sets (built once from the graph)
//! - `scheduled`: Shared `#[scheduled]` config/name/overlap emitters
//! - `transverse`: Shared bean/controller transverse emitters (scheduled
//!   sources, event subscribers, deco containers, dispatch wrappers,
//!   post-construct)
//! - `controller_impl`: Controller trait implementation generation

pub(crate) mod controller_impl;
pub(crate) mod decorators;
mod handlers;
pub(crate) mod scheduled;
pub(crate) mod transverse;
mod wrapping;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};

use crate::routes_parsing::RoutesImplDef;
use crate::types::MethodDecorators;

/// Main entry point: generate all code for a `#[routes]` impl block.
pub fn generate(def: &RoutesImplDef) -> TokenStream {
    let impl_block = wrapping::generate_impl_block(def);
    let handlers = handlers::generate_handlers(def);
    let controller_impl = controller_impl::generate_controller_impl(def);
    let anonymous_asserts = generate_anonymous_asserts(def);
    let fga_path_asserts = generate_fga_path_param_asserts(def);

    quote! {
        #impl_block
        #handlers
        #controller_impl
        #(#anonymous_asserts)*
        #fga_path_asserts
    }
}

/// If any method carries `#[anonymous]`, assert at compile time that the
/// controller declares a **required** struct-level identity to opt out of.
///
/// `#[routes]` cannot see the struct, so this is checked through the meta
/// module's `STRUCT_IDENTITY_IS_REQUIRED` const. Without a required identity
/// there is no fail-closed baseline: no identity means the routes are already
/// public, and an `Option<T>` identity never rejects — in both cases the
/// marker would be a silent no-op with placement side effects, so reject it.
/// One assert per controller (spanned to the first anonymous method) — every
/// marker on the controller shares the same root cause.
fn generate_anonymous_asserts(def: &RoutesImplDef) -> Vec<TokenStream> {
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);

    let first_anonymous = |decorators: &MethodDecorators, fn_item: &syn::ImplItemFn| {
        decorators.anonymous.then(|| fn_item.sig.ident.span())
    };

    def.route_methods
        .iter()
        .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item))
        .chain(
            def.sse_methods
                .iter()
                .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item)),
        )
        .chain(
            def.ws_methods
                .iter()
                .filter_map(|m| first_anonymous(&m.decorators, &m.fn_item)),
        )
        .next()
        .map(|span| {
            quote_spanned! { span =>
                const _: () = ::core::assert!(
                    #meta_mod::STRUCT_IDENTITY_IS_REQUIRED,
                    "#[anonymous] needs a required struct-level #[inject(identity)] field to opt out of: with no identity the routes are already public, and an Option<..> identity never rejects — the marker would be a no-op"
                );
            }
        })
        .into_iter()
        .collect()
}

/// Lift OpenFGA `from_path("param")` references to a compile error when the
/// named path parameter is absent from the route.
///
/// `#[guard(FgaCheck::relation(..).on(..).from_path("doc_id"))]` resolves its
/// object id from a `{doc_id}` path parameter at request time. When `doc_id`
/// matches no placeholder the check can only ever fail with a runtime
/// `ObjectResolutionFailed`; catching it here turns a deploy-time 400 into a
/// build error. The runtime resolver stays as the backstop for the dynamic
/// forms this static check cannot see.
///
/// Only the string-literal form rooted in an `FgaCheck` builder chain is
/// statically checkable. The route's true path is `PATH_PREFIX ++ method_path`,
/// but `#[routes]` only sees `method_path`; the prefix lives on the
/// `#[controller]` struct. So — like [`generate_anonymous_asserts`] — the check
/// is emitted as a `const _` assertion that reads `#meta_mod::PATH_PREFIX` and
/// evaluates the name against prefix + method path at const-eval time. This
/// covers prefix parameters (e.g. `#[controller(path = "/orgs/{org_id}")]`)
/// with no false positive. Non-literal `from_path(expr)` forms and any
/// non-`FgaCheck` builder fall through to the runtime backstop in `r2e-openfga`.
fn generate_fga_path_param_asserts(def: &RoutesImplDef) -> TokenStream {
    let meta_mod = format_ident!("__r2e_meta_{}", def.controller_name);

    // Collect (method path, referenced param literal) for every literal FGA
    // `from_path` in a guard/pre-guard expression on any HTTP/SSE/WS method.
    let mut refs: Vec<(String, syn::LitStr)> = Vec::new();
    let mut collect = |path: &str, decorators: &MethodDecorators| {
        for expr in decorators
            .guard_fns
            .iter()
            .chain(decorators.pre_auth_guard_fns.iter())
        {
            let mut lits = Vec::new();
            collect_fga_path_refs(expr, &mut lits);
            for lit in lits {
                refs.push((path.to_string(), lit));
            }
        }
    };
    for m in &def.route_methods {
        collect(&m.path, &m.decorators);
    }
    for m in &def.sse_methods {
        collect(&m.path, &m.decorators);
    }
    for m in &def.ws_methods {
        collect(&m.path, &m.decorators);
    }
    drop(collect);

    if refs.is_empty() {
        return TokenStream::new();
    }

    let asserts: Vec<TokenStream> = refs
        .iter()
        .map(|(path, lit)| {
            let param = lit.value();
            let declared = handlers::extract_route_path_param_names(path);
            let params_list = if declared.is_empty() {
                "none".to_string()
            } else {
                declared
                    .iter()
                    .map(|p| format!("`{{{p}}}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let raw_msg = format!(
                "FGA guard references path parameter `{param}`, but no `{{{param}}}` \
                 placeholder appears in the route path `{path}` (this method's params: \
                 {params_list}) or the controller's `path = \"...\"` prefix.\n\
                 help: check the spelling of `from_path(\"{param}\")` against the route's \
                 `{{...}}` placeholders",
            );
            // `assert!` parses its message as a format string, so every brace in
            // the rendered text must be doubled to avoid being read as a format
            // argument (the message names `{param}`-style placeholders literally).
            let msg = raw_msg.replace('{', "{{").replace('}', "}}");
            let const_fns = fga_param_const_fns();
            quote_spanned! { lit.span() =>
                const _: () = {
                    #const_fns
                    ::core::assert!(
                        __r2e_fga_route_has_param(#meta_mod::PATH_PREFIX, #path, #param),
                        #msg
                    );
                };
            }
        })
        .collect();

    quote! { #(#asserts)* }
}

/// The const-eval helpers used by [`generate_fga_path_param_asserts`]: scan an
/// Axum-style path for a `{name}` placeholder (allowing `{name:regex}` and the
/// `{*name}` catch-all form), and OR the method path with the controller prefix.
/// Emitted inside each assertion block so the block is self-contained and the
/// helper names never collide across controllers.
fn fga_param_const_fns() -> TokenStream {
    quote! {
        const fn __r2e_fga_seg_has_param(path: &str, name: &str) -> bool {
            let p = path.as_bytes();
            let n = name.as_bytes();
            let mut i = 0;
            while i < p.len() {
                if p[i] == b'{' {
                    let mut j = i + 1;
                    if j < p.len() && p[j] == b'*' {
                        j += 1;
                    }
                    let start = j;
                    while j < p.len() && p[j] != b'}' && p[j] != b':' {
                        j += 1;
                    }
                    if j - start == n.len() {
                        let mut k = 0;
                        let mut eq = true;
                        while k < n.len() {
                            if p[start + k] != n[k] {
                                eq = false;
                                break;
                            }
                            k += 1;
                        }
                        if eq {
                            return true;
                        }
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
            false
        }

        const fn __r2e_fga_route_has_param(
            prefix: ::core::option::Option<&str>,
            path: &str,
            name: &str,
        ) -> bool {
            if __r2e_fga_seg_has_param(path, name) {
                return true;
            }
            match prefix {
                ::core::option::Option::Some(p) => __r2e_fga_seg_has_param(p, name),
                ::core::option::Option::None => false,
            }
        }
    }
}

/// Collect literal `from_path("...")` references in an FGA guard expression.
///
/// Guards are single builder chains, so the common case is a direct
/// `FgaCheck::relation(..).on(..).from_path("id")`. The receiver is walked so a
/// nested chain (rare) is still covered; wrapper expressions (`(..)`, `&..`) are
/// unwrapped. Only method calls named `from_path` whose chain roots in an
/// `FgaCheck` path, taking a single string literal, are collected.
fn collect_fga_path_refs(expr: &syn::Expr, out: &mut Vec<syn::LitStr>) {
    if let syn::Expr::MethodCall(mc) = expr {
        if mc.method == "from_path" && chain_roots_in_fga(&mc.receiver) {
            if let Some(lit) = single_str_lit_arg(&mc.args) {
                out.push(lit);
            }
        }
        collect_fga_path_refs(&mc.receiver, out);
        return;
    }
    match expr {
        syn::Expr::Paren(e) => collect_fga_path_refs(&e.expr, out),
        syn::Expr::Group(e) => collect_fga_path_refs(&e.expr, out),
        syn::Expr::Reference(e) => collect_fga_path_refs(&e.expr, out),
        _ => {}
    }
}

/// Whether a builder chain's base expression is an `FgaCheck` path — the gate
/// that keeps this check scoped to OpenFGA guards and off any unrelated
/// `.from_path(..)` method a different type might expose.
fn chain_roots_in_fga(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::MethodCall(mc) => chain_roots_in_fga(&mc.receiver),
        syn::Expr::Call(call) => chain_roots_in_fga(&call.func),
        syn::Expr::Field(f) => chain_roots_in_fga(&f.base),
        syn::Expr::Paren(e) => chain_roots_in_fga(&e.expr),
        syn::Expr::Group(e) => chain_roots_in_fga(&e.expr),
        syn::Expr::Reference(e) => chain_roots_in_fga(&e.expr),
        syn::Expr::Path(p) => p.path.segments.iter().any(|s| s.ident == "FgaCheck"),
        _ => false,
    }
}

/// Return the single string-literal argument of a call, or `None` when the
/// argument list is not exactly one string literal (dynamic form → runtime
/// backstop).
fn single_str_lit_arg(
    args: &syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>,
) -> Option<syn::LitStr> {
    if args.len() != 1 {
        return None;
    }
    match args.first()? {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.clone()),
        _ => None,
    }
}
