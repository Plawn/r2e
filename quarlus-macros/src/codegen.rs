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
        .chain(def.config_fields.iter().map(|f| {
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
/// Route methods may get their body wrapped with interceptors.
/// Wrapping order (outermost first): logged → timed → rate_limited → cached → transactional → body.
fn generate_impl(def: &ControllerDef) -> TokenStream {
    let name = &def.name;

    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(|rm| generate_wrapped_method(rm))
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

/// Generate a method with the full interceptor chain applied.
fn generate_wrapped_method(rm: &RouteMethod) -> TokenStream {
    let has_interceptors = rm.transactional || rm.logged || rm.timed
        || rm.cached.is_some() || rm.rate_limited.is_some();

    if !has_interceptors {
        let f = &rm.fn_item;
        return quote! { #f };
    }

    let fn_item = &rm.fn_item;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let sig = &fn_item.sig;
    let fn_name_str = sig.ident.to_string();
    let original_body = &fn_item.block;

    // Start with the innermost body (the original code)
    let mut body = quote! { #original_body };

    // Layer 5 (innermost): transactional
    if rm.transactional {
        body = quote! {
            {
                let mut tx = self.pool.begin().await
                    .map_err(|__e| quarlus_core::AppError::Internal(__e.to_string()))?;
                let __tx_result = #body;
                match __tx_result {
                    Ok(__val) => {
                        tx.commit().await
                            .map_err(|__e| quarlus_core::AppError::Internal(__e.to_string()))?;
                        Ok(__val)
                    }
                    Err(__err) => Err(__err),
                }
            }
        };
    }

    // Layer 4: cached — wraps the body to check/store in a static TtlCache
    if let Some(ttl) = rm.cached {
        body = quote! {
            {
                use std::sync::OnceLock;
                static __CACHE: OnceLock<quarlus_core::TtlCache<String, String>> = OnceLock::new();
                let __cache = __CACHE.get_or_init(|| {
                    quarlus_core::TtlCache::new(std::time::Duration::from_secs(#ttl))
                });
                let __cache_key = format!("{}:{}", #fn_name_str, "default");
                if let Some(__cached) = __cache.get(&__cache_key) {
                    let __val: serde_json::Value = serde_json::from_str(&__cached)
                        .unwrap_or(serde_json::Value::String(__cached));
                    axum::Json(__val)
                } else {
                    let __result = #body;
                    if let Ok(__serialized) = serde_json::to_string(&__result.0) {
                        __cache.insert(__cache_key, __serialized);
                    }
                    __result
                }
            }
        };
    }

    // Layer 3: rate_limited — checks a static RateLimiter, returns 429 if exceeded
    if let Some(ref rl) = rm.rate_limited {
        let max = rl.max;
        let window = rl.window;
        body = quote! {
            {
                use std::sync::OnceLock;
                static __LIMITER: OnceLock<quarlus_core::RateLimiter<String>> = OnceLock::new();
                let __limiter = __LIMITER.get_or_init(|| {
                    quarlus_core::RateLimiter::new(#max, std::time::Duration::from_secs(#window))
                });
                let __key = format!("{}:global", #fn_name_str);
                if !__limiter.try_acquire(&__key) {
                    return Err(quarlus_core::AppError::Custom {
                        status: axum::http::StatusCode::TOO_MANY_REQUESTS,
                        body: serde_json::json!({ "error": "Rate limit exceeded" }),
                    });
                }
                #body
            }
        };
    }

    // Layer 2: timed
    if rm.timed {
        body = quote! {
            {
                let __start = std::time::Instant::now();
                let __result = #body;
                let __elapsed = __start.elapsed();
                tracing::info!(method = #fn_name_str, elapsed_ms = __elapsed.as_millis() as u64, "method execution time");
                __result
            }
        };
    }

    // Layer 1 (outermost): logged
    if rm.logged {
        body = quote! {
            {
                tracing::info!(method = #fn_name_str, "entering");
                let __result = #body;
                tracing::info!(method = #fn_name_str, "exiting");
                __result
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

    let config_inits: Vec<_> = def
        .config_fields
        .iter()
        .map(|f| {
            let n = &f.name;
            let key = &f.key;
            quote! {
                #n: {
                    let __cfg = <quarlus_core::QuarlusConfig as axum::extract::FromRef<#state_type>>::from_ref(&state);
                    __cfg.get(#key).unwrap_or_else(|e| panic!("Config key '{}' error: {}", #key, e))
                }
            }
        })
        .collect();

    let all_inits: Vec<_> = inject_inits
        .iter()
        .chain(identity_inits.iter())
        .chain(config_inits.iter())
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

            if rm.middleware_fns.is_empty() {
                quote! {
                    .route(#path, axum::routing::#method_fn(#handler_name))
                }
            } else {
                let layers: Vec<_> = rm.middleware_fns.iter().map(|mw_fn| {
                    quote! { .layer(axum::middleware::from_fn(#mw_fn)) }
                }).collect();

                quote! {
                    .route(
                        #path,
                        axum::routing::#method_fn(#handler_name)
                            #(#layers)*
                    )
                }
            }
        })
        .collect();

    let route_metadata_items: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| {
            let path = &rm.path;
            let method = rm.method.as_axum_method_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.roles.iter().map(|r| quote! { #r.to_string() }).collect();

            // Extract parameter info from the method signature
            let params: Vec<_> = rm.fn_item.sig.inputs.iter().filter_map(|arg| {
                if let syn::FnArg::Typed(pt) = arg {
                    let ty_str = quote!(#pt.ty).to_string();
                    // Detect Path<T> params
                    if ty_str.contains("Path") {
                        if let syn::Pat::TupleStruct(ts) = pt.pat.as_ref() {
                            if let Some(elem) = ts.elems.first() {
                                let param_name = quote!(#elem).to_string();
                                return Some(quote! {
                                    quarlus_core::openapi::ParamInfo {
                                        name: #param_name.to_string(),
                                        location: quarlus_core::openapi::ParamLocation::Path,
                                        param_type: "string".to_string(),
                                        required: true,
                                    }
                                });
                            }
                        }
                    }
                    None
                } else {
                    None
                }
            }).collect();

            quote! {
                quarlus_core::openapi::RouteInfo {
                    path: #path.to_string(),
                    method: #method.to_string(),
                    operation_id: #op_id.to_string(),
                    summary: None,
                    request_body_type: None,
                    response_type: None,
                    params: vec![#(#params),*],
                    roles: vec![#(#roles),*],
                }
            }
        })
        .collect();

    quote! {
        impl quarlus_core::Controller<#state_type> for #name {
            fn routes() -> axum::Router<#state_type> {
                axum::Router::new()
                    #(#route_registrations)*
            }

            fn route_metadata() -> Vec<quarlus_core::openapi::RouteInfo> {
                vec![#(#route_metadata_items),*]
            }
        }
    }
}
