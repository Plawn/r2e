//! Axum handler generation for route methods.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::type_utils::is_result_like;
use crate::types::*;

/// Generate all handler functions for a controller.
///
/// For each endpoint we emit one invocation function. HTTP/SSE route closures
/// bind the façade and call it directly; WebSocket additionally needs a thin
/// upgrade adapter. Guards, interceptors, managed resources, method invocation,
/// and response conversion are emitted only in the invocation function.
pub fn generate_handlers(def: &RoutesImplDef) -> TokenStream {
    let route_handlers: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| generate_single_handler(def, rm))
        .collect();

    let sse_handlers: Vec<_> = def
        .sse_methods
        .iter()
        .map(|sm| generate_sse_handler(def, sm))
        .collect();

    let ws_handlers: Vec<_> = def
        .ws_methods
        .iter()
        .map(|wm| generate_ws_handler(def, wm))
        .collect();

    quote! {
        #(#route_handlers)*
        #(#sse_handlers)*
        #(#ws_handlers)*
    }
}

fn invocation_ident_for(controller: &syn::Ident, method: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_invoke_{}_{}", controller, method)
}

/// The generic state ident shared by all state-generic generated items. Free
/// generated fns declare it themselves; items inside the `Controller` impl use
/// the impl's parameter of the same name.
pub(super) fn state_generic() -> syn::Ident {
    format_ident!("__R2eS")
}

/// The marker generic carried by the request-data extractor (a tuple of
/// per-field `FromRequestPartsVia` markers, opaque to `#[routes]`).
pub(super) fn data_marker() -> syn::Ident {
    format_ident!("__R2eMd")
}

/// The extraction marker for a route's param-level `#[inject(identity)]`.
/// Pascal-cased so the generated type parameter doesn't trip
/// `non_camel_case_types` in user crates.
pub(super) fn identity_marker_for(method: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eMp{}", crate::type_utils::to_pascal_case(&method.to_string()))
}

/// Bounds placed on the generic state by every generated item that touches it:
/// axum's `Router` requirements plus `BeanLookup`, the fixed vocabulary through
/// which guards, interceptors, and managed resources pull beans from the state.
pub(super) fn state_bounds(krate: &TokenStream) -> TokenStream {
    quote! { Clone + Send + Sync + 'static + #krate::BeanLookup }
}

/// The generated request façade type for a controller. Route/SSE/WS methods are
/// emitted on `impl __R2eRequest_<Name>`; handler invocation runs on a borrow of
/// it. Application/config fields and core helpers are reached through its
/// `Deref<Target = Core>`.
fn facade_ident_for(controller: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eRequest_{}", controller)
}

/// The generated request-data extractor type for a controller. Carries the
/// request-scoped values (identity + `#[inject(request)]`) and is bound into the
/// façade alongside the captured core `Arc`.
fn request_data_ident_for(controller: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eRequestData_{}", controller)
}

fn handler_ident_for(controller: &syn::Ident, method: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_{}_{}", controller, method)
}

/// Context for handler generation, containing names and identifiers.
struct HandlerContext<'a> {
    meta_mod: syn::Ident,
    invocation_name: syn::Ident,
    fn_name: &'a syn::Ident,
    fn_name_str: String,
    controller_name_str: String,
}

impl<'a> HandlerContext<'a> {
    fn new(def: &'a RoutesImplDef, rm: &'a RouteMethod) -> Self {
        let controller_name = &def.controller_name;
        let fn_name = &rm.fn_item.sig.ident;
        Self {
            meta_mod: format_ident!("__r2e_meta_{}", controller_name),
            invocation_name: invocation_ident_for(controller_name, fn_name),
            fn_name,
            fn_name_str: fn_name.to_string(),
            controller_name_str: controller_name.to_string(),
        }
    }
}

/// Extract handler parameters (everything except &self) with their indices.
fn extract_handler_params(rm: &RouteMethod) -> Vec<(usize, &syn::PatType)> {
    extract_sig_params(&rm.fn_item.sig)
}

/// Walk a method signature once and collect its typed params with indices,
/// dropping the `&self` receiver. Shared by HTTP / SSE / WS handler codegen.
fn extract_sig_params(sig: &syn::Signature) -> Vec<(usize, &syn::PatType)> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => Some(pat_type),
            syn::FnArg::Receiver(_) => None,
        })
        .enumerate()
        .collect()
}

/// Build handler parameter declarations, excluding managed params.
fn build_handler_params(
    extra_params: &[(usize, &syn::PatType)],
    managed_indices: &std::collections::HashSet<usize>,
) -> Vec<TokenStream> {
    extra_params
        .iter()
        .filter(|(i, _)| !managed_indices.contains(i))
        .map(|(i, pt)| {
            let arg_name = format_ident!("__arg_{}", i);
            let ty = &pt.ty;
            quote! { #arg_name: #ty }
        })
        .collect()
}

/// Build call arguments, substituting managed params with mutable refs.
fn build_call_args(
    extra_params: &[(usize, &syn::PatType)],
    managed_indices: &std::collections::HashSet<usize>,
) -> Vec<TokenStream> {
    extra_params
        .iter()
        .map(|(i, _)| {
            let arg_name = format_ident!("__arg_{}", i);
            if managed_indices.contains(i) {
                quote! { &mut #arg_name }
            } else {
                quote! { #arg_name }
            }
        })
        .collect()
}

/// Generate automatic validation calls for handler parameters.
///
/// Uses the autoref specialization trick: types deriving `garde::Validate` are
/// validated automatically; types without it compile to a no-op.
fn generate_validation_calls(
    extra_params: &[(usize, &syn::PatType)],
    managed_indices: &std::collections::HashSet<usize>,
    identity_param_index: Option<usize>,
    krate: &TokenStream,
) -> Vec<TokenStream> {
    extra_params
        .iter()
        .filter(|(i, _)| !managed_indices.contains(i) && Some(*i) != identity_param_index)
        .map(|(i, pt)| {
            let arg_name = format_ident!("__arg_{}", i);
            let validate_target = if is_wrapper_type(&pt.ty) {
                // For Json<T>, Query<T>, Path<T>, Form<T> → validate the inner .0
                quote! { &#arg_name.0 }
            } else {
                // For Params and other custom types → validate directly
                quote! { &#arg_name }
            };
            quote! {
                {
                    use #krate::validation::__DoValidate as _;
                    use #krate::validation::__SkipValidate as _;
                    if let Err(__validation_err) = (&#krate::validation::__AutoValidator(#validate_target)).__maybe_validate() {
                        return *__validation_err;
                    }
                }
            }
        })
        .collect()
}

/// Check if a type is a known Axum wrapper (Json, Query, Path, Form).
fn is_wrapper_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            return matches!(ident.as_str(), "Json" | "Query" | "Path" | "Form");
        }
    }
    false
}

struct PathParamSymbol {
    ident: syn::Ident,
    name: String,
    ty: syn::Type,
}

/// Extract `{name}` parameters from an Axum-style route path.
fn extract_route_path_param_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = path;

    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let raw = &after_open[..close];
        let name = raw
            .split(':')
            .next()
            .unwrap_or(raw)
            .trim()
            .trim_start_matches('*');
        if !name.is_empty() {
            names.push(name.to_string());
        }
        rest = &after_open[close + 1..];
    }

    names
}

fn path_wrapper_inner_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Path" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty.clone()),
        _ => None,
    })
}

fn flatten_path_inner_types(ty: &syn::Type) -> Vec<syn::Type> {
    match ty {
        syn::Type::Tuple(tuple) => tuple.elems.iter().cloned().collect(),
        other => vec![other.clone()],
    }
}

fn collect_pat_idents(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(ident) => {
            let mut name = ident.ident.to_string();
            if name != "_" {
                if let Some(stripped) = name.strip_prefix('_') {
                    if !stripped.is_empty() {
                        name = stripped.to_string();
                    }
                }
                out.push(name);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_pat_idents(elem, out);
            }
        }
        syn::Pat::TupleStruct(tuple_struct) => {
            for elem in &tuple_struct.elems {
                collect_pat_idents(elem, out);
            }
        }
        _ => {}
    }
}

fn path_extractor_info(sig: &syn::Signature) -> Option<(Vec<String>, Vec<syn::Type>)> {
    for (_, param) in extract_sig_params(sig) {
        let Some(inner_ty) = path_wrapper_inner_type(&param.ty) else {
            continue;
        };
        let mut pat_names = Vec::new();
        collect_pat_idents(&param.pat, &mut pat_names);
        return Some((pat_names, flatten_path_inner_types(&inner_ty)));
    }
    None
}

fn infer_path_param_symbols(path: &str, sig: &syn::Signature) -> Vec<PathParamSymbol> {
    let route_names = extract_route_path_param_names(path);
    let (pat_names, path_types) = path_extractor_info(sig).unwrap_or_default();

    let mut ordered_names = if pat_names.is_empty() {
        route_names.clone()
    } else {
        pat_names
    };

    for name in route_names {
        if !ordered_names.iter().any(|known| known == &name) {
            ordered_names.push(name);
        }
    }

    let fallback_ty: syn::Type = syn::parse_quote! { () };
    let mut symbols = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (index, name) in ordered_names.into_iter().enumerate() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Ok(ident) = syn::parse_str::<syn::Ident>(&name) else {
            continue;
        };
        let ty = path_types
            .get(index)
            .cloned()
            .unwrap_or_else(|| fallback_ty.clone());
        symbols.push(PathParamSymbol { ident, name, ty });
    }

    symbols
}

fn generate_path_param_module(
    path: &str,
    sig: &syn::Signature,
    krate: &TokenStream,
) -> TokenStream {
    let symbols = infer_path_param_symbols(path, sig);
    if symbols.is_empty() {
        return quote! {};
    }

    let consts: Vec<TokenStream> = symbols
        .iter()
        .map(|symbol| {
            let ident = &symbol.ident;
            let name = &symbol.name;
            let ty = &symbol.ty;
            quote! {
                pub const #ident: #krate::PathParam<#ty> = #krate::PathParam::new(#name);
            }
        })
        .collect();

    quote! {
        #[allow(non_snake_case)]
        #[allow(non_upper_case_globals)]
        mod path {
            use super::*;
            #(#consts)*
        }
    }
}

/// Generate guard check statements.
fn generate_guard_checks(guard_fns: &[syn::Expr], krate: &TokenStream) -> Vec<TokenStream> {
    guard_fns
        .iter()
        .map(|guard_expr| {
            quote! {
                if let Err(__resp) = #krate::Guard::check(
                    &#guard_expr,
                    &__state,
                    &__guard_ctx,
                ).await {
                    return __resp;
                }
            }
        })
        .collect()
}

/// Generate guard context construction based on identity source.
fn generate_guard_context(
    ctx: &HandlerContext,
    rm: &RouteMethod,
    krate: &TokenStream,
) -> TokenStream {
    let fn_name_str = &ctx.fn_name_str;
    let controller_name_str = &ctx.controller_name_str;
    let meta_mod = &ctx.meta_mod;

    if let Some(ref id_param) = rm.identity_param {
        // Case A: param-level identity
        let arg_name = format_ident!("__arg_{}", id_param.index);
        let identity_expr = if id_param.is_optional {
            quote! { #arg_name.as_ref() }
        } else {
            quote! { Some(&#arg_name) }
        };
        quote! {
            let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
            let __guard_ctx = #krate::GuardContext {
                method_name: #fn_name_str,
                controller_name: #controller_name_str,
                headers: &__headers,
                uri: &__uri,
                path_params: __path_params,
                identity: #identity_expr,
            };
        }
    } else {
        // Case B: struct-level identity or no identity. Both adapters have
        // already normalized their controller source to `&Controller`.
        quote! {
            let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
            let __guard_ctx = #krate::GuardContext {
                method_name: #fn_name_str,
                controller_name: #controller_name_str,
                headers: &__headers,
                uri: &__uri,
                path_params: __path_params,
                identity: #meta_mod::guard_identity(__ctrl),
            };
        }
    }
}

/// Generate managed resource acquisition statements.
///
/// Uses `quote_spanned!` so any trait-bound error (e.g. `T: ManagedResource<State>`
/// not satisfied) points at the user's own `&mut T` parameter type rather than
/// the macro-expanded handler body.
fn generate_managed_acquire(
    rm: &RouteMethod,
    krate: &TokenStream,
) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            let ty_span = ty.span();
            quote_spanned! { ty_span =>
                let mut #arg_name = match <#ty as #krate::ManagedResource<__R2eS>>::acquire(&__state).await {
                    Ok(__r) => __r,
                    Err(__e) => return __e.into(),
                };
            }
        })
        .collect()
}

/// Generate managed resource release statements (in reverse order).
fn generate_managed_release(
    rm: &RouteMethod,
    krate: &TokenStream,
) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .rev()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            let ty_span = ty.span();
            quote_spanned! { ty_span =>
                if let Err(__e) = <#ty as #krate::ManagedResource<__R2eS>>::release(#arg_name, __success).await {
                    return __e.into();
                }
            }
        })
        .collect()
}

/// Generate the body and release logic for managed resources.
fn generate_body_and_release(
    call_expr: &TokenStream,
    managed_release: &[TokenStream],
    has_managed: bool,
    is_result: bool,
    krate: &TokenStream,
) -> TokenStream {
    if has_managed {
        if is_result {
            quote! {
                let __result = #call_expr;
                let __success = __result.is_ok();
                #(#managed_release)*
                #krate::http::response::IntoResponse::into_response(__result)
            }
        } else {
            quote! {
                let __result = #call_expr;
                let __success = true;
                #(#managed_release)*
                #krate::http::response::IntoResponse::into_response(__result)
            }
        }
    } else {
        quote! {
            #krate::http::response::IntoResponse::into_response(#call_expr)
        }
    }
}

/// Check if a route method has interceptors (method-level or controller-level).
fn has_interceptors(def: &RoutesImplDef, rm: &RouteMethod) -> bool {
    !rm.decorators.intercept_fns.is_empty() || !def.controller_intercepts.is_empty()
}

/// Wrap a body expression with the interceptor chain at handler level.
///
/// Uses `__state_ref: &S` (which is `Copy`) to construct `InterceptorContext`
/// at each layer. The `move || async move { ... }` closures capture
/// `__state_ref` by copy and other variables by move.
fn wrap_with_handler_interceptors(
    body: TokenStream,
    fn_name_str: &str,
    controller_name_str: &str,
    def: &RoutesImplDef,
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

    // Wrap from innermost interceptor to second interceptor (skip outermost)
    for intercept_expr in all_intercepts[1..].iter().rev() {
        wrapped = quote! {
            move || async move {
                let __interceptor = #intercept_expr;
                #krate::Interceptor::around(
                    &__interceptor,
                    #krate::InterceptorContext {
                        method_name: #fn_name_str,
                        controller_name: #controller_name_str,
                        state: __state_ref,
                    },
                    #wrapped
                ).await
            }
        };
    }

    // Apply the outermost interceptor directly (not wrapped in a closure)
    let outermost = &all_intercepts[0];
    quote! {
        {
            let __state_ref: &_ = &__state;
            let __interceptor = #outermost;
            #krate::Interceptor::around(
                &__interceptor,
                #krate::InterceptorContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    state: __state_ref,
                },
                #wrapped
            ).await
        }
    }
}

/// Generate managed resource acquisition using `__state_ref` (for use inside interceptor closures).
fn generate_managed_acquire_ref(
    rm: &RouteMethod,
    krate: &TokenStream,
) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            let ty_span = ty.span();
            quote_spanned! { ty_span =>
                let mut #arg_name = match <#ty as #krate::ManagedResource<__R2eS>>::acquire(__state_ref).await {
                    Ok(__r) => __r,
                    Err(__e) => return __e.into(),
                };
            }
        })
        .collect()
}

/// Generate a single Axum handler function.
///
/// # Case matrix
///
/// The handler shape depends on which features are active:
///
/// | Case | Guards/Managed | Interceptors | Validation | Return type      |
/// |------|----------------|--------------|------------|------------------|
/// | 1a   | No             | No           | No         | Handler's own    |
/// | 1b   | No             | No           | Yes        | Response         |
/// | 2a   | No             | Yes          | No         | Handler's own    |
/// | 2b   | No             | Yes          | Yes        | Response         |
/// | 3    | Yes            | Optional     | Optional   | Response         |
///
/// # Design invariant
///
/// When interceptors are present, they **always wrap the raw handler call** — the
/// `IntoResponse::into_response()` conversion is applied *after* the outermost
/// interceptor. This ensures interceptors see the handler's native type (`Json<T>`,
/// `Result<Json<T>, E>`, etc.) and type-constrained interceptors like `Cache`
/// (which requires `R: Cacheable`) work correctly alongside guards/roles.
///
/// **Exception:** when `#[managed]` params are present with interceptors (Case 3,
/// `has_managed` branch), the managed lifecycle wraps `into_response` inside the
/// interceptor closure because release errors must convert to `Response`. This means
/// type-constrained interceptors don't work with `#[managed]` params.
fn generate_single_handler(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let krate = r2e_core_path();
    let ctx = HandlerContext::new(def, rm);
    let controller_name = &def.controller_name;
    let return_type = &rm.fn_item.sig.output;

    let extra_params = extract_handler_params(rm);
    let managed_indices: std::collections::HashSet<usize> =
        rm.managed_params.iter().map(|mp| mp.index).collect();

    let handler_extra_params = build_handler_params(&extra_params, &managed_indices);
    let call_args = build_call_args(&extra_params, &managed_indices);

    let call_expr = if rm.fn_item.sig.asyncness.is_some() {
        let fn_name = ctx.fn_name;
        quote! { __ctrl.#fn_name(#(#call_args),*).await }
    } else {
        let fn_name = ctx.fn_name;
        quote! { __ctrl.#fn_name(#(#call_args),*) }
    };

    let has_guards = !rm.decorators.guard_fns.is_empty();
    let has_managed = !rm.managed_params.is_empty();
    let has_intercepts = has_interceptors(def, rm);
    let needs_response = has_guards || has_managed;
    let needs_state = needs_response || has_intercepts;

    let invocation_name = &ctx.invocation_name;
    let fn_name_str = &ctx.fn_name_str;
    let controller_name_str = &ctx.controller_name_str;

    // Generate validation calls for all non-managed, non-identity parameters
    let identity_param_index = rm.identity_param.as_ref().map(|p| p.index);
    let validation_calls = generate_validation_calls(
        &extra_params,
        &managed_indices,
        identity_param_index,
        &krate,
    );
    let has_validation = !validation_calls.is_empty();

    let mut invocation_prefix_params = Vec::new();

    if needs_state {
        invocation_prefix_params.push(quote! { __state: __R2eS });
    }
    if has_guards {
        invocation_prefix_params.push(quote! { __headers: #krate::http::HeaderMap });
        invocation_prefix_params.push(quote! { __uri: #krate::http::Uri });
        invocation_prefix_params
            .push(quote! { __raw_path_params: #krate::http::extract::RawPathParams });
    }

    let invocation_extra_params = &handler_extra_params;

    let (invocation_return, invocation_body) = if !needs_state && !has_validation {
        // Case 1a: Simple handler — no guards, no managed, no interceptors, no validation
        (quote! { #return_type }, quote! { #call_expr })
    } else if !needs_state && has_validation {
        // Case 1b: Simple handler with validation — returns Response
        (
            quote! { -> #krate::http::response::Response },
            quote! {
                #(#validation_calls)*
                #krate::http::response::IntoResponse::into_response(#call_expr)
            },
        )
    } else if has_intercepts && !needs_response && !has_validation {
        // Case 2a: Interceptors only, no validation — returns method's own type
        let interceptor_body = wrap_with_handler_interceptors(
            call_expr,
            fn_name_str,
            controller_name_str,
            def,
            &rm.decorators.intercept_fns,
            &krate,
        );

        (quote! { #return_type }, interceptor_body)
    } else if has_intercepts && !needs_response && has_validation {
        // Case 2b: Interceptors + validation — returns Response
        // Apply into_response AFTER the interceptor chain so interceptors
        // see the handler's raw return type (e.g. Json<T>), not Response.
        let interceptor_body = wrap_with_handler_interceptors(
            call_expr.clone(),
            fn_name_str,
            controller_name_str,
            def,
            &rm.decorators.intercept_fns,
            &krate,
        );
        let interceptor_body = quote! {
            #krate::http::response::IntoResponse::into_response(#interceptor_body)
        };

        (
            quote! { -> #krate::http::response::Response },
            quote! {
                #(#validation_calls)*
                #interceptor_body
            },
        )
    } else {
        // Case 3: Complex handler — returns Response (guards and/or managed, optionally interceptors)
        let guard_checks = generate_guard_checks(&rm.decorators.guard_fns, &krate);
        let path_param_module = if has_guards {
            generate_path_param_module(&rm.path, &rm.fn_item.sig, &krate)
        } else {
            quote! {}
        };
        let guard_context_construction = if has_guards {
            generate_guard_context(&ctx, rm, &krate)
        } else {
            quote! {}
        };

        // Build the inner body (after guards, including managed lifecycle)
        let inner_body = if has_intercepts {
            // Wrap the managed lifecycle + call in interceptors.
            // Inside the interceptor closure, use __state_ref for acquire.
            let is_result = is_result_type(return_type);
            if has_managed {
                let managed_acquire_ref = generate_managed_acquire_ref(rm, &krate);
                let managed_release = generate_managed_release(rm, &krate);
                let body_and_release = generate_body_and_release(
                    &call_expr,
                    &managed_release,
                    true,
                    is_result,
                    &krate,
                );
                let managed_body = quote! {
                    #(#managed_acquire_ref)*
                    #body_and_release
                };
                wrap_with_handler_interceptors(
                    managed_body,
                    fn_name_str,
                    controller_name_str,
                    def,
                    &rm.decorators.intercept_fns,
                    &krate,
                )
            } else {
                // Apply into_response AFTER the interceptor chain so interceptors
                // see the handler's raw return type (e.g. Json<T>), not Response.
                // This fixes #[intercept(Cache)] + #[roles] (or any guard) combinations.
                let interceptor_body = wrap_with_handler_interceptors(
                    call_expr.clone(),
                    fn_name_str,
                    controller_name_str,
                    def,
                    &rm.decorators.intercept_fns,
                    &krate,
                );
                quote! {
                    #krate::http::response::IntoResponse::into_response(#interceptor_body)
                }
            }
        } else {
            // No interceptors — original behavior
            let managed_acquire = generate_managed_acquire(rm, &krate);
            let managed_release = generate_managed_release(rm, &krate);
            let is_result = is_result_type(return_type);
            let body_and_release = generate_body_and_release(
                &call_expr,
                &managed_release,
                has_managed,
                is_result,
                &krate,
            );
            quote! {
                #(#managed_acquire)*
                #body_and_release
            }
        };

        (
            quote! { -> #krate::http::response::Response },
            quote! {
                #guard_context_construction
                #path_param_module
                #(#guard_checks)*
                #(#validation_calls)*
                #inner_body
            },
        )
    };

    let facade_name = facade_ident_for(controller_name);
    let (fn_generics, fn_where) = if needs_state {
        let sb = state_bounds(&krate);
        let managed_bounds: Vec<TokenStream> = rm
            .managed_params
            .iter()
            .map(|mp| {
                let ty = crate::type_utils::staticize_lifetimes(&mp.ty);
                quote! { #ty: #krate::ManagedResource<__R2eS> }
            })
            .collect();
        (
            quote! { <__R2eS> },
            quote! { where __R2eS: #sb, #(#managed_bounds,)* },
        )
    } else {
        (quote! {}, quote! {})
    };
    quote! {
        #[allow(non_snake_case)]
        async fn #invocation_name #fn_generics(
            #(#invocation_prefix_params,)*
            __ctrl: &#facade_name,
            #(#invocation_extra_params,)*
        ) #invocation_return #fn_where {
            #invocation_body
        }
    }
}

fn is_result_type(return_type: &syn::ReturnType) -> bool {
    match return_type {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => is_result_like(ty),
    }
}

// ── SSE handler generation ───────────────────────────────────────────────

/// Generate a handler function for an `#[sse("/path")]` method.
fn generate_sse_handler(def: &RoutesImplDef, sm: &SseMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &sm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let invocation_name = invocation_ident_for(controller_name, fn_name);

    let fn_name_str = fn_name.to_string();
    let controller_name_str = controller_name.to_string();

    // Extra params (excluding &self)
    let extra_params = extract_sig_params(&sm.fn_item.sig);

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

    let call_expr = if sm.fn_item.sig.asyncness.is_some() {
        quote! { __ctrl.#fn_name(#(#call_args),*).await }
    } else {
        quote! { __ctrl.#fn_name(#(#call_args),*) }
    };

    // Keep-alive wrapping
    let keep_alive_expr = match sm.keep_alive {
        SseKeepAlive::Default => {
            quote! {
                #krate::http::response::Sse::new(__stream)
                    .keep_alive(#krate::http::response::SseKeepAlive::default())
            }
        }
        SseKeepAlive::Interval(secs) => {
            quote! {
                #krate::http::response::Sse::new(__stream)
                    .keep_alive(
                        #krate::http::response::SseKeepAlive::new()
                            .interval(std::time::Duration::from_secs(#secs))
                    )
            }
        }
        SseKeepAlive::Disabled => {
            quote! { #krate::http::response::Sse::new(__stream) }
        }
    };

    let has_guards = !sm.decorators.guard_fns.is_empty();

    let mut invocation_prefix_params = Vec::new();

    let (invocation_return, invocation_body) = if !has_guards {
        (
            quote! { -> impl #krate::http::response::IntoResponse },
            quote! {
                let __stream = #call_expr;
                #keep_alive_expr
            },
        )
    } else {
        invocation_prefix_params.push(quote! { __state: __R2eS });
        invocation_prefix_params.push(quote! { __headers: #krate::http::HeaderMap });
        invocation_prefix_params.push(quote! { __uri: #krate::http::Uri });
        invocation_prefix_params
            .push(quote! { __raw_path_params: #krate::http::extract::RawPathParams });

        let guard_checks = generate_guard_checks(&sm.decorators.guard_fns, &krate);
        let path_param_module = generate_path_param_module(&sm.path, &sm.fn_item.sig, &krate);

        let guard_context = if let Some(ref id_param) = sm.identity_param {
            let arg_name = format_ident!("__arg_{}", id_param.index);
            let identity_expr = if id_param.is_optional {
                quote! { #arg_name.as_ref() }
            } else {
                quote! { Some(&#arg_name) }
            };
            quote! {
                let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
                let __guard_ctx = #krate::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    uri: &__uri,
                    path_params: __path_params,
                    identity: #identity_expr,
                };
            }
        } else {
            quote! {
                let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
                let __guard_ctx = #krate::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    uri: &__uri,
                    path_params: __path_params,
                    identity: #meta_mod::guard_identity(__ctrl),
                };
            }
        };

        (
            quote! { -> #krate::http::response::Response },
            quote! {
                #path_param_module
                #guard_context
                #(#guard_checks)*
                let __stream = #call_expr;
                #krate::http::response::IntoResponse::into_response(#keep_alive_expr)
            },
        )
    };

    let invocation_extra_params = &handler_extra_params;

    let facade_name = facade_ident_for(controller_name);
    let (fn_generics, fn_where) = if has_guards {
        let sb = state_bounds(&krate);
        (quote! { <__R2eS> }, quote! { where __R2eS: #sb })
    } else {
        (quote! {}, quote! {})
    };
    quote! {
        #[allow(non_snake_case)]
        async fn #invocation_name #fn_generics(
            #(#invocation_prefix_params,)*
            __ctrl: &#facade_name,
            #(#invocation_extra_params,)*
        ) #invocation_return #fn_where {
            #invocation_body
        }
    }
}

// ── WS handler generation ────────────────────────────────────────────────

/// Generate a handler function for a `#[ws("/path")]` method.
fn generate_ws_handler(def: &RoutesImplDef, wm: &WsMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &wm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let facade_name = facade_ident_for(controller_name);
    let invocation_name = invocation_ident_for(controller_name, fn_name);
    let preflight_name = format_ident!("__r2e_preflight_{}_{}", controller_name, fn_name);

    let fn_name_str = fn_name.to_string();
    let controller_name_str = controller_name.to_string();

    // Collect all typed params, excluding WsStream/WebSocket
    let extra_params = extract_sig_params(&wm.fn_item.sig);

    let ws_param_index = wm.ws_param.as_ref().map(|p| p.index);

    // Handler params: skip the WsStream/WebSocket param (it comes from on_upgrade)
    let handler_extra_params: Vec<_> = extra_params
        .iter()
        .filter(|(i, _)| Some(*i) != ws_param_index)
        .map(|(i, pt)| {
            let arg_name = format_ident!("__arg_{}", i);
            let ty = &pt.ty;
            quote! { #arg_name: #ty }
        })
        .collect();
    let forwarded_args: Vec<_> = extra_params
        .iter()
        .filter(|(i, _)| Some(*i) != ws_param_index)
        .map(|(i, _)| {
            let arg_name = format_ident!("__arg_{}", i);
            quote! { #arg_name }
        })
        .collect();

    let has_guards = !wm.decorators.guard_fns.is_empty();

    // Build the shared post-upgrade invocation body. Controller ownership
    // remains in the thin adapter's `on_upgrade` closure; this function only
    // receives a borrow while the session setup/method invocation runs.
    let invocation_body = if let Some(ref ws_p) = wm.ws_param {
        // Pattern 1: WsStream or WebSocket parameter
        let call_args: Vec<_> = extra_params
            .iter()
            .map(|(i, _)| {
                let arg_name = format_ident!("__arg_{}", i);
                if Some(*i) == ws_param_index {
                    if ws_p.is_ws_stream {
                        quote! { __ws_stream }
                    } else {
                        quote! { __socket }
                    }
                } else {
                    quote! { #arg_name }
                }
            })
            .collect();

        let ws_setup = if ws_p.is_ws_stream {
            quote! { let __ws_stream = #krate::ws::WsStream::new(__socket); }
        } else {
            quote! {}
        };

        let call = if wm.fn_item.sig.asyncness.is_some() {
            quote! { __ctrl.#fn_name(#(#call_args),*).await; }
        } else {
            quote! { __ctrl.#fn_name(#(#call_args),*); }
        };

        quote! {
            #ws_setup
            #call
        }
    } else {
        // Pattern 2: no WsStream param → method returns impl WsHandler
        let call_args: Vec<_> = extra_params
            .iter()
            .map(|(i, _)| {
                let arg_name = format_ident!("__arg_{}", i);
                quote! { #arg_name }
            })
            .collect();

        let call = if wm.fn_item.sig.asyncness.is_some() {
            quote! { let __handler = __ctrl.#fn_name(#(#call_args),*).await; }
        } else {
            quote! { let __handler = __ctrl.#fn_name(#(#call_args),*); }
        };

        quote! {
            #call
            #krate::ws::run_ws_handler(#krate::ws::WsStream::new(__socket), __handler).await;
        }
    };

    let ws_state_bounds = state_bounds(&krate);
    let (preflight, preflight_call) = if has_guards {
        let guard_checks: Vec<TokenStream> = wm
            .decorators
            .guard_fns
            .iter()
            .map(|guard_expr| {
                quote! {
                    if let Err(__resp) = #krate::Guard::check(
                        &#guard_expr,
                        &__state,
                        &__guard_ctx,
                    ).await {
                        return Err(__resp);
                    }
                }
            })
            .collect();
        let path_param_module = generate_path_param_module(&wm.path, &wm.fn_item.sig, &krate);

        let (identity_decl, identity_call, identity_expr) =
            if let Some(ref id_param) = wm.identity_param {
                let arg_name = format_ident!("__arg_{}", id_param.index);
                let identity_ty = extra_params
                    .iter()
                    .find(|(index, _)| *index == id_param.index)
                    .map(|(_, param)| &param.ty)
                    .expect("identity parameter must be present in the method signature");
                let identity_expr = if id_param.is_optional {
                    quote! { __identity.as_ref() }
                } else {
                    quote! { Some(__identity) }
                };
                (
                    quote! { __identity: &#identity_ty },
                    quote! { &#arg_name },
                    identity_expr,
                )
            } else {
                (
                    quote! {},
                    quote! {},
                    quote! { #meta_mod::guard_identity(__ctrl) },
                )
            };

        let guard_context = quote! {
                let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
                let __guard_ctx = #krate::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    uri: &__uri,
                    path_params: __path_params,
                    identity: #identity_expr,
                };
        };

        (
            quote! {
                #[allow(non_snake_case)]
                async fn #preflight_name<__R2eS>(
                    __state: __R2eS,
                    __headers: #krate::http::HeaderMap,
                    __uri: #krate::http::Uri,
                    __raw_path_params: #krate::http::extract::RawPathParams,
                    __ctrl: &#facade_name,
                    #identity_decl
                ) -> Result<(), #krate::http::response::Response>
                where
                    __R2eS: #ws_state_bounds,
                {
                    #path_param_module
                    #guard_context
                    #(#guard_checks)*
                    Ok(())
                }
            },
            quote! {
                if let Err(__response) = #preflight_name(
                    __state,
                    __headers,
                    __uri,
                    __raw_path_params,
                    __ctrl_for_guard,
                    #identity_call
                ).await {
                    return __response;
                }
            },
        )
    } else {
        (quote! {}, quote! {})
    };

    let handler_name = handler_ident_for(controller_name, fn_name);
    let guard_params = if has_guards {
        quote! {
            #krate::http::extract::State(__state): #krate::http::extract::State<__R2eS>,
            __headers: #krate::http::HeaderMap,
            __uri: #krate::http::Uri,
            __raw_path_params: #krate::http::extract::RawPathParams,
        }
    } else {
        quote! {}
    };
    let guard_controller_borrow = if has_guards {
        quote! { let __ctrl_for_guard = &__facade; }
    } else {
        quote! {}
    };

    let (wrapper_generics, wrapper_where) = if has_guards {
        let sb = state_bounds(&krate);
        (quote! { <__R2eS> }, quote! { where __R2eS: #sb })
    } else {
        (quote! {}, quote! {})
    };
    quote! {
        #[allow(non_snake_case)]
        async fn #invocation_name(
            __ctrl: &#facade_name,
            #(#handler_extra_params,)*
            __socket: #krate::http::ws::WebSocket,
        ) {
            #invocation_body
        }

        #preflight
        #[allow(non_snake_case)]
        async fn #handler_name #wrapper_generics(
            #guard_params
            __facade: #facade_name,
            #(#handler_extra_params,)*
            __ws_upgrade: #krate::http::ws::WebSocketUpgrade,
        ) -> #krate::http::response::Response #wrapper_where {
            // Guard checks borrow the façade; the upgrade callback then owns it
            // for the whole socket lifetime (façade owns its Arc + request data,
            // so nothing is borrowed across the upgrade boundary).
            #guard_controller_borrow
            #preflight_call
            #krate::http::response::IntoResponse::into_response(
                __ws_upgrade.on_upgrade(move |__socket| async move {
                    #invocation_name(
                        &__facade,
                        #(#forwarded_args,)*
                        __socket,
                    ).await
                })
            )
        }
    }
}

// ── Application-controller closure helpers ──────────────────────────────
//
// These build the `move |...| async move { __r2e_<Name>_<m>(...) }`
// closures registered by the state-aware route builder for non-identity
// controllers. The closure captures the controller `Arc` and forwards the
// request-extracted parameters to the common hidden handler wrapper.

/// Build the Axum-extractable params + matching call args for a closure
/// wrapping the application-scoped HTTP handler.
fn route_axum_params_and_args(
    rm: &RouteMethod,
    needs_state: bool,
    has_guards: bool,
    krate: &TokenStream,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let identity_index = rm.identity_param.as_ref().map(|p| p.index);
    let identity_marker = identity_marker_for(&rm.fn_item.sig.ident);
    let mut params: Vec<TokenStream> = Vec::new();
    let mut args: Vec<TokenStream> = Vec::new();
    if needs_state {
        params.push(quote! {
            #krate::http::extract::State(__state): #krate::http::extract::State<__R2eS>
        });
        args.push(quote! { __state });
    }
    if has_guards {
        params.push(quote! { __headers: #krate::http::HeaderMap });
        params.push(quote! { __uri: #krate::http::Uri });
        params.push(quote! { __raw_path_params: #krate::http::extract::RawPathParams });
        args.push(quote! { __headers });
        args.push(quote! { __uri });
        args.push(quote! { __raw_path_params });
    }

    let extra_params = extract_handler_params(rm);
    let managed_indices: std::collections::HashSet<usize> =
        rm.managed_params.iter().map(|mp| mp.index).collect();
    for (i, pt) in extra_params
        .iter()
        .filter(|(i, _)| !managed_indices.contains(i))
    {
        let arg = format_ident!("__arg_{}", i);
        let ty = &pt.ty;
        if Some(*i) == identity_index {
            // Param-level identity: extracted through `FromRequestPartsVia`
            // (bean-backed, witness in the marker) and unwrapped before the
            // invocation call, so the route method keeps the plain type.
            params.push(quote! { #arg: #krate::extract::Via<#ty, #identity_marker> });
            args.push(quote! { #arg.0 });
        } else {
            params.push(quote! { #arg: #ty });
            args.push(quote! { #arg });
        }
    }
    (params, args)
}

/// Generate the closure expression that registers an application-scoped HTTP
/// handler for a route.
pub(super) fn generate_route_closure(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &rm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let data_name = request_data_ident_for(controller_name);
    let invocation = invocation_ident_for(controller_name, fn_name);

    let has_guards = !rm.decorators.guard_fns.is_empty();
    let has_managed = !rm.managed_params.is_empty();
    let has_intercepts = has_interceptors(def, rm);
    let needs_state = has_guards || has_managed || has_intercepts;

    let (closure_params, fwd_args) =
        route_axum_params_and_args(rm, needs_state, has_guards, &krate);

    // Splice __ctrl into the inner-handler call after the axum-extracted
    // prefix (state + optional guard-context params), matching the inner
    // handler's signature: `(State?, [HeaderMap, Uri, RawPathParams]?, __ctrl, extras...)`.
    let prefix_len = if has_guards {
        if needs_state {
            4
        } else {
            3
        }
    } else if needs_state {
        1
    } else {
        0
    };
    let (prefix, suffix) = fwd_args.split_at(prefix_len);
    // One per-request `Arc` increment: axum clones the `Fn`-once closure per
    // request (cloning `__core_capture`), then this body moves that clone into
    // `bind_request`. There is no second explicit `.clone()`.
    let md = data_marker();
    quote! {
        {
            let __core_capture = __ctrl.clone();
            move |__r2e_data: #data_name<#md>, #(#closure_params),*| {
                async move {
                    let __facade = #meta_mod::bind_request(__core_capture, __r2e_data);
                    #invocation(
                        #(#prefix,)*
                        &__facade,
                        #(#suffix),*
                    ).await
                }
            }
        }
    }
}

/// Same as `generate_route_closure`, but for `#[sse]` endpoints. SSE
/// handlers always omit the `__state` parameter when guards are not present.
pub(super) fn generate_sse_closure(def: &RoutesImplDef, sm: &SseMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &sm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let data_name = request_data_ident_for(controller_name);
    let invocation = invocation_ident_for(controller_name, fn_name);
    let has_guards = !sm.decorators.guard_fns.is_empty();

    let mut closure_params: Vec<TokenStream> = Vec::new();
    let mut fwd_args: Vec<TokenStream> = Vec::new();
    if has_guards {
        closure_params.push(quote! {
            #krate::http::extract::State(__state): #krate::http::extract::State<__R2eS>
        });
        closure_params.push(quote! { __headers: #krate::http::HeaderMap });
        closure_params.push(quote! { __uri: #krate::http::Uri });
        closure_params.push(quote! { __raw_path_params: #krate::http::extract::RawPathParams });
        fwd_args.push(quote! { __state });
        fwd_args.push(quote! { __headers });
        fwd_args.push(quote! { __uri });
        fwd_args.push(quote! { __raw_path_params });
    }
    let identity_index = sm.identity_param.as_ref().map(|p| p.index);
    let identity_marker = identity_marker_for(&sm.fn_item.sig.ident);
    for (i, pt) in extract_sig_params(&sm.fn_item.sig) {
        let arg = format_ident!("__arg_{}", i);
        let ty = &pt.ty;
        if Some(i) == identity_index {
            closure_params.push(quote! { #arg: #krate::extract::Via<#ty, #identity_marker> });
            fwd_args.push(quote! { #arg.0 });
        } else {
            closure_params.push(quote! { #arg: #ty });
            fwd_args.push(quote! { #arg });
        }
    }

    let prefix_len = if has_guards { 4 } else { 0 };
    let (prefix, suffix) = fwd_args.split_at(prefix_len);
    let md = data_marker();
    quote! {
        {
            let __core_capture = __ctrl.clone();
            move |__r2e_data: #data_name<#md>, #(#closure_params),*| {
                async move {
                    let __facade = #meta_mod::bind_request(__core_capture, __r2e_data);
                    #invocation(
                        #(#prefix,)*
                        &__facade,
                        #(#suffix),*
                    ).await
                }
            }
        }
    }
}

/// Same captured-core adapter pattern for `#[ws]` endpoints. The WS
/// handler always ends with a `WebSocketUpgrade` parameter, which we surface
/// as the closure's final parameter.
pub(super) fn generate_ws_closure(def: &RoutesImplDef, wm: &WsMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &wm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let data_name = request_data_ident_for(controller_name);
    let inner = handler_ident_for(controller_name, fn_name);
    let has_guards = !wm.decorators.guard_fns.is_empty();
    let ws_param_index = wm.ws_param.as_ref().map(|p| p.index);

    let mut closure_params: Vec<TokenStream> = Vec::new();
    let mut fwd_args: Vec<TokenStream> = Vec::new();
    if has_guards {
        closure_params.push(quote! {
            __state_ext: #krate::http::extract::State<__R2eS>
        });
        closure_params.push(quote! { __headers: #krate::http::HeaderMap });
        closure_params.push(quote! { __uri: #krate::http::Uri });
        closure_params.push(quote! { __raw_path_params: #krate::http::extract::RawPathParams });
        fwd_args.push(quote! { __state_ext });
        fwd_args.push(quote! { __headers });
        fwd_args.push(quote! { __uri });
        fwd_args.push(quote! { __raw_path_params });
    }
    let identity_index = wm.identity_param.as_ref().map(|p| p.index);
    let identity_marker = identity_marker_for(&wm.fn_item.sig.ident);
    for (i, pt) in extract_sig_params(&wm.fn_item.sig) {
        if Some(i) == ws_param_index {
            continue;
        }
        let arg = format_ident!("__arg_{}", i);
        let ty = &pt.ty;
        if Some(i) == identity_index {
            closure_params.push(quote! { #arg: #krate::extract::Via<#ty, #identity_marker> });
            fwd_args.push(quote! { #arg.0 });
        } else {
            closure_params.push(quote! { #arg: #ty });
            fwd_args.push(quote! { #arg });
        }
    }
    closure_params.push(quote! { __ws_upgrade: #krate::http::ws::WebSocketUpgrade });
    fwd_args.push(quote! { __ws_upgrade });

    let md = data_marker();
    let prefix_len = if has_guards { 4 } else { 0 };
    let (prefix, suffix) = fwd_args.split_at(prefix_len);
    quote! {
        {
            let __core_capture = __ctrl.clone();
            move |__r2e_data: #data_name<#md>, #(#closure_params),*| {
                async move {
                    #inner(
                        #(#prefix,)*
                        #meta_mod::bind_request(__core_capture, __r2e_data),
                        #(#suffix),*
                    ).await
                }
            }
        }
    }
}
