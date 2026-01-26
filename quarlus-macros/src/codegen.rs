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
///
/// Wrapping order (outermost first):
///   logged → timed → user-defined interceptors → cached → cache_invalidate → transactional → body.
fn generate_impl(def: &ControllerDef) -> TokenStream {
    let name = &def.name;

    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(|rm| generate_wrapped_method(rm, def))
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

// ---------------------------------------------------------------------------
// Helper: convert parsing::LogLevel to runtime token stream
// ---------------------------------------------------------------------------

fn log_level_tokens(level: LogLevel) -> TokenStream {
    match level {
        LogLevel::Trace => quote! { quarlus_core::interceptors::LogLevel::Trace },
        LogLevel::Debug => quote! { quarlus_core::interceptors::LogLevel::Debug },
        LogLevel::Info => quote! { quarlus_core::interceptors::LogLevel::Info },
        LogLevel::Warn => quote! { quarlus_core::interceptors::LogLevel::Warn },
        LogLevel::Error => quote! { quarlus_core::interceptors::LogLevel::Error },
    }
}

// ---------------------------------------------------------------------------
// Helper: extract first identifier from a pattern (for cache key generation)
// ---------------------------------------------------------------------------

fn extract_first_ident(pat: &syn::Pat) -> Option<syn::Ident> {
    match pat {
        syn::Pat::Ident(pi) => Some(pi.ident.clone()),
        syn::Pat::TupleStruct(ts) => ts.elems.first().and_then(extract_first_ident),
        syn::Pat::Tuple(t) => t.elems.first().and_then(extract_first_ident),
        syn::Pat::Reference(r) => extract_first_ident(&r.pat),
        _ => None,
    }
}

fn extract_param_bindings(rm: &RouteMethod) -> Vec<syn::Ident> {
    rm.fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => extract_first_ident(&pt.pat),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helper: generate cache key expression
// ---------------------------------------------------------------------------

fn identity_field_name(def: &ControllerDef) -> &syn::Ident {
    assert!(
        !def.identity_fields.is_empty(),
        "cached(key = \"user\") or cached(key = \"user_params\") requires an #[identity] field on the controller"
    );
    &def.identity_fields[0].name
}

fn generate_cache_key_expr(
    fn_name: &str,
    cache_key: &CacheKey,
    rm: &RouteMethod,
    def: &ControllerDef,
) -> TokenStream {
    match cache_key {
        CacheKey::Default => {
            quote! { format!("{}:default", #fn_name) }
        }
        CacheKey::Params => {
            let bindings = extract_param_bindings(rm);
            if bindings.is_empty() {
                quote! { format!("{}:default", #fn_name) }
            } else {
                quote! {
                    {
                        let mut __key = format!("{}", #fn_name);
                        #(
                            __key.push(':');
                            __key.push_str(&format!("{:?}", #bindings));
                        )*
                        __key
                    }
                }
            }
        }
        CacheKey::User => {
            let identity_name = identity_field_name(def);
            quote! { format!("{}:user:{}", #fn_name, self.#identity_name.sub) }
        }
        CacheKey::UserParams => {
            let identity_name = identity_field_name(def);
            let bindings = extract_param_bindings(rm);
            if bindings.is_empty() {
                quote! { format!("{}:user:{}", #fn_name, self.#identity_name.sub) }
            } else {
                quote! {
                    {
                        let mut __key = format!("{}:user:{}", #fn_name, self.#identity_name.sub);
                        #(
                            __key.push(':');
                            __key.push_str(&format!("{:?}", #bindings));
                        )*
                        __key
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Method wrapping with interceptors
// ---------------------------------------------------------------------------

/// Generate a method with the full interceptor chain applied.
///
/// Inline wrappers (innermost): transactional, cache_invalidate
/// Trait-based interceptors (via Interceptor::around): cached, user-defined, timed, logged
/// Rate limiting is handled at the handler level (generate_single_handler).
fn generate_wrapped_method(rm: &RouteMethod, def: &ControllerDef) -> TokenStream {
    let has_interceptors = rm.transactional.is_some()
        || rm.logged.is_some()
        || rm.timed.is_some()
        || rm.cached.is_some()
        || !rm.intercept_fns.is_empty()
        || !rm.cache_invalidate.is_empty();

    if !has_interceptors {
        let f = &rm.fn_item;
        return quote! { #f };
    }

    let fn_item = &rm.fn_item;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let sig = &fn_item.sig;
    let fn_name_str = sig.ident.to_string();
    let controller_name_str = def.name.to_string();
    let original_body = &fn_item.block;

    // Start with the innermost body (the original code)
    let mut body: TokenStream = quote! { #original_body };

    // -----------------------------------------------------------------------
    // Inline wrapper (innermost): transactional
    // -----------------------------------------------------------------------
    if let Some(ref tx_config) = rm.transactional {
        let pool_field = format_ident!("{}", tx_config.pool_field);
        body = quote! {
            {
                let mut tx = self.#pool_field.begin().await
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

    // -----------------------------------------------------------------------
    // Inline wrapper: cache_invalidate (after body)
    // -----------------------------------------------------------------------
    for group in &rm.cache_invalidate {
        body = quote! {
            {
                let __result = #body;
                quarlus_core::CacheRegistry::invalidate(#group);
                __result
            }
        };
    }

    // -----------------------------------------------------------------------
    // Trait-based interceptors (via Interceptor::around)
    // Build from innermost to outermost.
    // -----------------------------------------------------------------------

    let has_trait_interceptors = rm.logged.is_some()
        || rm.timed.is_some()
        || rm.cached.is_some()
        || !rm.intercept_fns.is_empty();

    // Layer: cached (innermost trait-based interceptor)
    if let Some(ref cached_config) = rm.cached {
        let ttl = cached_config.ttl;
        let key_expr = generate_cache_key_expr(&fn_name_str, &cached_config.key, rm, def);

        let cache_init = if let Some(ref group) = cached_config.group {
            quote! {
                let __cache = quarlus_core::CacheRegistry::get_or_create(
                    #group,
                    std::time::Duration::from_secs(#ttl),
                );
            }
        } else {
            quote! {
                use std::sync::OnceLock;
                static __CACHE: OnceLock<quarlus_core::TtlCache<String, String>> = OnceLock::new();
                let __cache = __CACHE.get_or_init(|| {
                    quarlus_core::TtlCache::new(std::time::Duration::from_secs(#ttl))
                }).clone();
            }
        };

        body = quote! {
            {
                #cache_init
                let __cached = quarlus_core::interceptors::Cached {
                    cache: __cache,
                    key: #key_expr,
                };
                quarlus_core::Interceptor::around(&__cached, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    // Layer: user-defined interceptors (in reverse order so first declared is outermost)
    for intercept_fn in rm.intercept_fns.iter().rev() {
        body = quote! {
            {
                let __interceptor = #intercept_fn;
                quarlus_core::Interceptor::around(&__interceptor, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    // Layer: timed
    if let Some(ref timed_config) = rm.timed {
        let level = log_level_tokens(timed_config.level);
        let threshold = match timed_config.threshold_ms {
            Some(ms) => quote! { Some(#ms) },
            None => quote! { None },
        };
        body = quote! {
            {
                let __timed = quarlus_core::interceptors::Timed {
                    level: #level,
                    threshold_ms: #threshold,
                };
                quarlus_core::Interceptor::around(&__timed, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    // Layer: logged (outermost trait-based interceptor)
    if let Some(ref logged_config) = rm.logged {
        let level = log_level_tokens(logged_config.level);
        body = quote! {
            {
                let __logged = quarlus_core::interceptors::Logged {
                    level: #level,
                };
                quarlus_core::Interceptor::around(&__logged, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    // Wrap with InterceptorContext creation if any trait-based interceptors used
    if has_trait_interceptors {
        body = quote! {
            {
                let __ctx = quarlus_core::InterceptorContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                };
                #body
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

// ---------------------------------------------------------------------------
// Handler generation
// ---------------------------------------------------------------------------

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
    let fn_name_str = fn_name.to_string();

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

    // Determine if handler needs to return Response (for guard/rate-limit short-circuit)
    let needs_response = !rm.roles.is_empty() || rm.rate_limited.is_some();

    if !needs_response {
        // Simple handler: returns the method's own type
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
        // Guarded handler: returns Response to allow short-circuit
        let identity_name = if !def.identity_fields.is_empty() {
            Some(&def.identity_fields[0].name)
        } else {
            None
        };

        // --- Rate limit guard ---
        let rate_limit_guard = if let Some(ref rl) = rm.rate_limited {
            let max = rl.max;
            let window = rl.window;

            let key_expr = match rl.key {
                RateLimitKey::Global => {
                    quote! { format!("{}:global", #fn_name_str) }
                }
                RateLimitKey::User => {
                    let id_name = identity_name
                        .expect("rate_limited(key = \"user\") requires an #[identity] field");
                    quote! { format!("{}:user:{}", #fn_name_str, #id_name.sub) }
                }
                RateLimitKey::Ip => {
                    quote! {
                        {
                            let __ip = __headers
                                .get("x-forwarded-for")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|v| v.split(',').next())
                                .map(|s| s.trim().to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            format!("{}:ip:{}", #fn_name_str, __ip)
                        }
                    }
                }
            };

            quote! {
                {
                    use std::sync::OnceLock;
                    static __LIMITER: OnceLock<quarlus_core::RateLimiter<String>> = OnceLock::new();
                    let __limiter = __LIMITER.get_or_init(|| {
                        quarlus_core::RateLimiter::new(#max, std::time::Duration::from_secs(#window))
                    });
                    let __rl_key = #key_expr;
                    if !__limiter.try_acquire(&__rl_key) {
                        return axum::response::IntoResponse::into_response((
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            axum::Json(serde_json::json!({ "error": "Rate limit exceeded" })),
                        ));
                    }
                }
            }
        } else {
            quote! {}
        };

        // --- Roles guard ---
        let roles_guard = if !rm.roles.is_empty() {
            let role_strs = &rm.roles;
            let id_name = identity_name
                .expect("#[roles] requires an #[identity] field");
            quote! {
                if !#id_name.has_any_role(&[#(#role_strs),*]) {
                    return axum::response::IntoResponse::into_response(
                        quarlus_core::AppError::Forbidden("Insufficient roles".into()),
                    );
                }
            }
        } else {
            quote! {}
        };

        // Extra parameter: HeaderMap (needed for IP-based rate limiting)
        let needs_headers = matches!(
            rm.rate_limited.as_ref().map(|rl| &rl.key),
            Some(RateLimitKey::Ip)
        );
        let headers_param = if needs_headers {
            quote! { __headers: axum::http::HeaderMap, }
        } else {
            quote! {}
        };

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                axum::extract::State(state): axum::extract::State<#state_type>,
                #(#identity_params,)*
                #headers_param
                #(#handler_extra_params,)*
            ) -> axum::response::Response {
                #rate_limit_guard
                #roles_guard
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
