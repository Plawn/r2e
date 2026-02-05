//! Axum handler generation for route methods.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::quarlus_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

/// Generate all handler functions for a controller.
pub fn generate_handlers(def: &RoutesImplDef) -> TokenStream {
    let handlers: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| generate_single_handler(def, rm))
        .collect();

    quote! { #(#handlers)* }
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
            meta_mod: format_ident!("__quarlus_meta_{}", controller_name),
            extractor_name: format_ident!("__QuarlusExtract_{}", controller_name),
            handler_name: format_ident!("__quarlus_{}_{}", controller_name, fn_name),
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

/// Generate a single Axum handler function.
fn generate_single_handler(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let krate = quarlus_core_path();
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

    let has_guards = !rm.guard_fns.is_empty();
    let has_managed = !rm.managed_params.is_empty();
    let needs_response = has_guards || has_managed;

    let extractor_name = &ctx.extractor_name;
    let handler_name = &ctx.handler_name;
    let meta_mod = &ctx.meta_mod;

    if !needs_response {
        // Simple handler: returns the method's own type
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
    } else {
        // Complex handler: returns Response
        let guard_checks = generate_guard_checks(&rm.guard_fns, &krate);
        let guard_context_construction = if has_guards {
            generate_guard_context(&ctx, rm, &krate)
        } else {
            quote! {}
        };

        let managed_acquire = generate_managed_acquire(rm, meta_mod, &krate);
        let managed_release = generate_managed_release(rm, meta_mod, &krate);
        let is_result = is_result_type(return_type);
        let body_and_release =
            generate_body_and_release(&call_expr, &managed_release, has_managed, is_result, &krate);

        let guard_params = if has_guards {
            quote! {
                __headers: #krate::http::HeaderMap,
                __uri: #krate::http::Uri,
                __raw_path_params: #krate::http::extract::RawPathParams,
            }
        } else {
            quote! {}
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
                let __ctrl = __ctrl_ext.0;
                #(#managed_acquire)*
                #body_and_release
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
