//! Axum handler generation for route methods.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

/// Generate all handler functions for a controller.
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

/// Context for handler generation, containing names and identifiers.
struct HandlerContext<'a> {
    meta_mod: syn::Ident,
    extractor_name: syn::Ident,
    handler_name: syn::Ident,
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
            extractor_name: format_ident!("__R2eExtract_{}", controller_name),
            handler_name: format_ident!("__r2e_{}_{}", controller_name, fn_name),
            fn_name,
            fn_name_str: fn_name.to_string(),
            controller_name_str: controller_name.to_string(),
        }
    }
}

/// Extract handler parameters (everything except &self) with their indices.
fn extract_handler_params(rm: &RouteMethod) -> Vec<(usize, &syn::PatType)> {
    rm.fn_item
        .sig
        .inputs
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
                        return __validation_err;
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
            return matches!(
                ident.as_str(),
                "Json" | "Query" | "Path" | "Form"
            );
        }
    }
    false
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
fn generate_guard_context(ctx: &HandlerContext, rm: &RouteMethod, krate: &TokenStream) -> TokenStream {
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
        // Case B: struct-level identity or no identity
        quote! {
            let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
            let __guard_ctx = #krate::GuardContext {
                method_name: #fn_name_str,
                controller_name: #controller_name_str,
                headers: &__headers,
                uri: &__uri,
                path_params: __path_params,
                identity: #meta_mod::guard_identity(&__ctrl_ext.0),
            };
        }
    }
}

/// Generate managed resource acquisition statements.
fn generate_managed_acquire(rm: &RouteMethod, meta_mod: &syn::Ident, krate: &TokenStream) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            quote! {
                let mut #arg_name = match <#ty as #krate::ManagedResource<#meta_mod::State>>::acquire(&__state).await {
                    Ok(__r) => __r,
                    Err(__e) => return __e.into(),
                };
            }
        })
        .collect()
}

/// Generate managed resource release statements (in reverse order).
fn generate_managed_release(rm: &RouteMethod, meta_mod: &syn::Ident, krate: &TokenStream) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .rev()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            quote! {
                if let Err(__e) = <#ty as #krate::ManagedResource<#meta_mod::State>>::release(#arg_name, __success).await {
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
fn generate_managed_acquire_ref(rm: &RouteMethod, meta_mod: &syn::Ident, krate: &TokenStream) -> Vec<TokenStream> {
    rm.managed_params
        .iter()
        .map(|mp| {
            let arg_name = format_ident!("__arg_{}", mp.index);
            let ty = &mp.ty;
            quote! {
                let mut #arg_name = match <#ty as #krate::ManagedResource<#meta_mod::State>>::acquire(__state_ref).await {
                    Ok(__r) => __r,
                    Err(__e) => return __e.into(),
                };
            }
        })
        .collect()
}

/// Generate a single Axum handler function.
fn generate_single_handler(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let krate = r2e_core_path();
    let ctx = HandlerContext::new(def, rm);
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

    let extractor_name = &ctx.extractor_name;
    let handler_name = &ctx.handler_name;
    let meta_mod = &ctx.meta_mod;
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

    if !needs_state && !has_validation {
        // Case 1a: Simple handler — no guards, no managed, no interceptors, no validation
        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) #return_type {
                let __ctrl = __ctrl_ext.0;
                #call_expr
            }
        }
    } else if !needs_state && has_validation {
        // Case 1b: Simple handler with validation — returns Response
        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> #krate::http::response::Response {
                #(#validation_calls)*
                let __ctrl = __ctrl_ext.0;
                #krate::http::response::IntoResponse::into_response(#call_expr)
            }
        }
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

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                #krate::http::extract::State(__state): #krate::http::extract::State<#meta_mod::State>,
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) #return_type {
                let __ctrl = __ctrl_ext.0;
                #interceptor_body
            }
        }
    } else if has_intercepts && !needs_response && has_validation {
        // Case 2b: Interceptors + validation — returns Response
        let inner_call = quote! {
            #krate::http::response::IntoResponse::into_response(#call_expr)
        };
        let interceptor_body = wrap_with_handler_interceptors(
            inner_call,
            fn_name_str,
            controller_name_str,
            def,
            &rm.decorators.intercept_fns,
            &krate,
        );

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                #krate::http::extract::State(__state): #krate::http::extract::State<#meta_mod::State>,
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> #krate::http::response::Response {
                #(#validation_calls)*
                let __ctrl = __ctrl_ext.0;
                #interceptor_body
            }
        }
    } else {
        // Case 3: Complex handler — returns Response (guards and/or managed, optionally interceptors)
        let guard_checks = generate_guard_checks(&rm.decorators.guard_fns, &krate);
        let guard_context_construction = if has_guards {
            generate_guard_context(&ctx, rm, &krate)
        } else {
            quote! {}
        };

        let guard_params = if has_guards {
            quote! {
                __headers: #krate::http::HeaderMap,
                __uri: #krate::http::Uri,
                __raw_path_params: #krate::http::extract::RawPathParams,
            }
        } else {
            quote! {}
        };

        // Build the inner body (after guards, including managed lifecycle)
        let inner_body = if has_intercepts {
            // Wrap the managed lifecycle + call in interceptors.
            // Inside the interceptor closure, use __state_ref for acquire.
            let is_result = is_result_type(return_type);
            if has_managed {
                let managed_acquire_ref = generate_managed_acquire_ref(rm, meta_mod, &krate);
                let managed_release = generate_managed_release(rm, meta_mod, &krate);
                let body_and_release =
                    generate_body_and_release(&call_expr, &managed_release, true, is_result, &krate);
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
                let wrapped_call = quote! {
                    #krate::http::response::IntoResponse::into_response(#call_expr)
                };
                wrap_with_handler_interceptors(
                    wrapped_call,
                    fn_name_str,
                    controller_name_str,
                    def,
                    &rm.decorators.intercept_fns,
                    &krate,
                )
            }
        } else {
            // No interceptors — original behavior
            let managed_acquire = generate_managed_acquire(rm, meta_mod, &krate);
            let managed_release = generate_managed_release(rm, meta_mod, &krate);
            let is_result = is_result_type(return_type);
            let body_and_release =
                generate_body_and_release(&call_expr, &managed_release, has_managed, is_result, &krate);
            quote! {
                #(#managed_acquire)*
                #body_and_release
            }
        };

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                #krate::http::extract::State(__state): #krate::http::extract::State<#meta_mod::State>,
                #guard_params
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> #krate::http::response::Response {
                #guard_context_construction
                #(#guard_checks)*
                #(#validation_calls)*
                let __ctrl = __ctrl_ext.0;
                #inner_body
            }
        }
    }
}

/// Checks if a return type annotation is a Result type.
fn is_result_type(return_type: &syn::ReturnType) -> bool {
    if let syn::ReturnType::Type(_, ty) = return_type {
        if let syn::Type::Path(type_path) = ty.as_ref() {
            if let Some(segment) = type_path.path.segments.last() {
                return segment.ident == "Result";
            }
        }
    }
    false
}

// ── SSE handler generation ───────────────────────────────────────────────

/// Generate a handler function for an `#[sse("/path")]` method.
fn generate_sse_handler(def: &RoutesImplDef, sm: &SseMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_name = &sm.fn_item.sig.ident;
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let extractor_name = format_ident!("__R2eExtract_{}", controller_name);
    let handler_name = format_ident!("__r2e_{}_{}", controller_name, fn_name);

    let fn_name_str = fn_name.to_string();
    let controller_name_str = controller_name.to_string();

    // Extra params (excluding &self)
    let extra_params: Vec<(usize, &syn::PatType)> = sm
        .fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(pt),
            _ => None,
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

    if !has_guards {
        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> impl #krate::http::response::IntoResponse {
                let __ctrl = __ctrl_ext.0;
                let __stream = #call_expr;
                #keep_alive_expr
            }
        }
    } else {
        let guard_checks = generate_guard_checks(&sm.decorators.guard_fns, &krate);

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
                    identity: #meta_mod::guard_identity(&__ctrl_ext.0),
                };
            }
        };

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                #krate::http::extract::State(__state): #krate::http::extract::State<#meta_mod::State>,
                __headers: #krate::http::HeaderMap,
                __uri: #krate::http::Uri,
                __raw_path_params: #krate::http::extract::RawPathParams,
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> #krate::http::response::Response {
                #guard_context
                #(#guard_checks)*
                let __ctrl = __ctrl_ext.0;
                let __stream = #call_expr;
                #krate::http::response::IntoResponse::into_response(#keep_alive_expr)
            }
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
    let extractor_name = format_ident!("__R2eExtract_{}", controller_name);
    let handler_name = format_ident!("__r2e_{}_{}", controller_name, fn_name);

    let fn_name_str = fn_name.to_string();
    let controller_name_str = controller_name.to_string();

    // Collect all typed params, excluding WsStream/WebSocket
    let extra_params: Vec<(usize, &syn::PatType)> = wm
        .fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(pt),
            _ => None,
        })
        .enumerate()
        .collect();

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

    let has_guards = !wm.decorators.guard_fns.is_empty();

    // Build the on_upgrade closure body
    let upgrade_body = if let Some(ref ws_p) = wm.ws_param {
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

    if !has_guards {
        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
                __ws_upgrade: #krate::http::ws::WebSocketUpgrade,
            ) -> #krate::http::response::Response {
                __ws_upgrade.on_upgrade(move |__socket| async move {
                    let __ctrl = __ctrl_ext.0;
                    #upgrade_body
                }).into_response()
            }
        }
    } else {
        let guard_checks = generate_guard_checks(&wm.decorators.guard_fns, &krate);

        let guard_context = if let Some(ref id_param) = wm.identity_param {
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
                    identity: #meta_mod::guard_identity(&__ctrl_ext.0),
                };
            }
        };

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                #krate::http::extract::State(__state): #krate::http::extract::State<#meta_mod::State>,
                __headers: #krate::http::HeaderMap,
                __uri: #krate::http::Uri,
                __raw_path_params: #krate::http::extract::RawPathParams,
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
                __ws_upgrade: #krate::http::ws::WebSocketUpgrade,
            ) -> #krate::http::response::Response {
                #guard_context
                #(#guard_checks)*
                __ws_upgrade.on_upgrade(move |__socket| async move {
                    let __ctrl = __ctrl_ext.0;
                    #upgrade_body
                }).into_response()
            }
        }
    }
}
