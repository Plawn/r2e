use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parsing::*;

pub fn generate(def: &ControllerDef) -> TokenStream {
    let struct_def = generate_struct(def);
    let impl_block = generate_impl(def);
    let handlers = generate_handlers(def);
    let controller_impl = generate_controller_impl(def);
    let scheduled_impl = generate_scheduled_impl(def);

    quote! {
        #struct_def
        #impl_block
        #handlers
        #controller_impl
        #scheduled_impl
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
        #[allow(dead_code)]
        pub struct #name {
            #(#fields),*
        }
    }
}

/// Generate `impl Name { ... }` with all original methods.
/// Route methods may get their body wrapped with interceptors.
///
/// Wrapping order (outermost first):
///   intercept chain (first declared = outermost) → transactional → body.
fn generate_impl(def: &ControllerDef) -> TokenStream {
    let name = &def.name;

    let route_fns: Vec<TokenStream> = def
        .route_methods
        .iter()
        .map(|rm| generate_wrapped_method(rm, def))
        .collect();

    let consumer_fns: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let f = &cm.fn_item;
            quote! { #f }
        })
        .collect();

    let scheduled_fns: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| generate_wrapped_scheduled_method(sm, def))
        .collect();

    let other_fns: Vec<_> = def.other_methods.iter().collect();

    if route_fns.is_empty() && consumer_fns.is_empty() && scheduled_fns.is_empty() && other_fns.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #name {
                #(#route_fns)*
                #(#consumer_fns)*
                #(#scheduled_fns)*
                #(#other_fns)*
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Method wrapping with interceptors
// ---------------------------------------------------------------------------

/// Generate a method with the full interceptor chain applied.
///
/// Inline wrappers (innermost): transactional
/// Trait-based interceptors (via Interceptor::around): all `#[intercept(...)]` entries
fn generate_wrapped_method(rm: &RouteMethod, def: &ControllerDef) -> TokenStream {
    let has_interceptors = rm.transactional.is_some()
        || !rm.intercept_fns.is_empty()
        || !def.controller_intercepts.is_empty();

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
    // Trait-based interceptors (via Interceptor::around)
    // Build from innermost to outermost (reverse order so first declared = outermost).
    // -----------------------------------------------------------------------
    let all_intercepts: Vec<&syn::Expr> = def.controller_intercepts.iter()
        .chain(rm.intercept_fns.iter())
        .collect();

    if !all_intercepts.is_empty() {
        for intercept_expr in all_intercepts.iter().rev() {
            body = quote! {
                {
                    let __interceptor = #intercept_expr;
                    quarlus_core::Interceptor::around(&__interceptor, __ctx, move || async move {
                        #body
                    }).await
                }
            };
        }

        // Wrap with InterceptorContext creation
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
// Scheduled method wrapping
// ---------------------------------------------------------------------------

/// Generate a scheduled method with interceptor chain applied (no transactional/guard).
fn generate_wrapped_scheduled_method(sm: &ScheduledMethod, def: &ControllerDef) -> TokenStream {
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
    let controller_name_str = def.name.to_string();
    let original_body = &fn_item.block;

    let mut body: TokenStream = quote! { #original_body };

    let all_intercepts: Vec<&syn::Expr> = def
        .controller_intercepts
        .iter()
        .chain(sm.intercept_fns.iter())
        .collect();

    for intercept_expr in all_intercepts.iter().rev() {
        body = quote! {
            {
                let __interceptor = #intercept_expr;
                quarlus_core::Interceptor::around(&__interceptor, __ctx, move || async move {
                    #body
                }).await
            }
        };
    }

    body = quote! {
        {
            let __ctx = quarlus_core::InterceptorContext {
                method_name: #fn_name_str,
                controller_name: #controller_name_str,
            };
            #body
        }
    };

    quote! {
        #(#attrs)*
        #vis #sig {
            #body
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduled controller impl generation
// ---------------------------------------------------------------------------

/// Generate `impl ScheduledController<T> for Name` if any `#[scheduled]` methods exist.
fn generate_scheduled_impl(def: &ControllerDef) -> TokenStream {
    if def.scheduled_methods.is_empty() {
        return quote! {};
    }

    let name = &def.name;
    let state_type = &def.state_type;
    let controller_name_str = name.to_string();

    let task_registrations: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| {
            let fn_name = &sm.fn_item.sig.ident;
            let fn_name_str = fn_name.to_string();
            let task_name = match &sm.config.name {
                Some(n) => n.clone(),
                None => format!("{}_{}", controller_name_str, fn_name_str),
            };

            let schedule_expr = if let Some(every) = sm.config.every {
                if let Some(delay) = sm.config.initial_delay {
                    quote! {
                        quarlus_scheduler::Schedule::EveryDelay {
                            interval: std::time::Duration::from_secs(#every),
                            initial_delay: std::time::Duration::from_secs(#delay),
                        }
                    }
                } else {
                    quote! {
                        quarlus_scheduler::Schedule::Every(
                            std::time::Duration::from_secs(#every)
                        )
                    }
                }
            } else {
                let cron_expr = sm.config.cron.as_ref().unwrap();
                quote! {
                    quarlus_scheduler::Schedule::Cron(#cron_expr.to_string())
                }
            };

            // Build controller field initialisers (inject + config only, no identity)
            let inject_inits: Vec<TokenStream> = def
                .injected_fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote! { #n: __state.#n.clone() }
                })
                .collect();

            let config_inits: Vec<TokenStream> = def
                .config_fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    let key = &f.key;
                    quote! {
                        #n: {
                            let __cfg = <quarlus_core::QuarlusConfig as axum::extract::FromRef<#state_type>>::from_ref(&__state);
                            __cfg.get(#key).unwrap_or_else(|e| panic!("Config key '{}' error: {}", #key, e))
                        }
                    }
                })
                .collect();

            let all_inits: Vec<&TokenStream> = inject_inits
                .iter()
                .chain(config_inits.iter())
                .collect();

            let is_async = sm.fn_item.sig.asyncness.is_some();
            let call_expr = if is_async {
                quote! { __ctrl.#fn_name().await }
            } else {
                quote! { __ctrl.#fn_name() }
            };

            quote! {
                __scheduler.add_task(quarlus_scheduler::ScheduledTask {
                    name: #task_name.to_string(),
                    schedule: #schedule_expr,
                    task: Box::new(move |__state: #state_type| {
                        Box::pin(async move {
                            let __ctrl = #name {
                                #(#all_inits,)*
                            };
                            quarlus_scheduler::ScheduledResult::log_if_err(
                                #call_expr,
                                #task_name,
                            );
                        })
                    }),
                });
            }
        })
        .collect();

    quote! {
        impl quarlus_scheduler::ScheduledController<#state_type> for #name {
            fn register_scheduled_tasks(
                __scheduler: &mut quarlus_scheduler::Scheduler<#state_type>,
            ) {
                #(#task_registrations)*
            }
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

    // Determine if handler needs to return Response (for guard short-circuit)
    let needs_response = !rm.guard_fns.is_empty();

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
        let controller_name_str = controller_name.to_string();

        let identity_name = if !def.identity_fields.is_empty() {
            Some(&def.identity_fields[0].name)
        } else {
            None
        };

        let (identity_sub_expr, identity_roles_expr) = if let Some(id_name) = identity_name {
            (
                quote! { Some(&#id_name.sub) },
                quote! { Some(&#id_name.roles) },
            )
        } else {
            (quote! { None }, quote! { None })
        };

        let guard_checks: Vec<TokenStream> = rm
            .guard_fns
            .iter()
            .map(|guard_expr| {
                quote! {
                    if let Err(__resp) = quarlus_core::Guard::check(
                        &#guard_expr,
                        &state,
                        &__guard_ctx,
                    ) {
                        return __resp;
                    }
                }
            })
            .collect();

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                axum::extract::State(state): axum::extract::State<#state_type>,
                #(#identity_params,)*
                __headers: axum::http::HeaderMap,
                #(#handler_extra_params,)*
            ) -> axum::response::Response {
                let __guard_ctx = quarlus_core::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    identity_sub: #identity_sub_expr,
                    identity_roles: #identity_roles_expr,
                };
                #(#guard_checks)*
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

    let tag_name = name.to_string();
    let route_metadata_items: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| {
            let path = if let Some(ref prefix) = def.prefix {
                format!("{}{}", prefix, rm.path)
            } else {
                rm.path.clone()
            };
            let path = &path;
            let method = rm.method.as_axum_method_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

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

            // Detect request body type from Json<T> or Validated<T> extractors
            let body_info: Option<(String, syn::Type)> = rm.fn_item.sig.inputs.iter().find_map(|arg| {
                if let syn::FnArg::Typed(pt) = arg {
                    extract_body_type_info(&pt.ty)
                } else {
                    None
                }
            });
            let (body_type_token, body_schema_token) = match &body_info {
                Some((name, inner_ty)) => (
                    quote! { Some(#name.to_string()) },
                    quote! {
                        Some({
                            let __schema = schemars::schema_for!(#inner_ty);
                            serde_json::to_value(__schema).unwrap()
                        })
                    },
                ),
                None => (
                    quote! { None },
                    quote! { None },
                ),
            };

            quote! {
                quarlus_core::openapi::RouteInfo {
                    path: #path.to_string(),
                    method: #method.to_string(),
                    operation_id: #op_id.to_string(),
                    summary: None,
                    request_body_type: #body_type_token,
                    request_body_schema: #body_schema_token,
                    response_type: None,
                    params: vec![#(#params),*],
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                }
            }
        })
        .collect();

    // --- Consumer registrations ---
    let consumer_registrations: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let bus_field = format_ident!("{}", cm.bus_field);
            let event_type = &cm.event_type;
            let fn_name = &cm.fn_item.sig.ident;
            let controller_name = &def.name;

            let inject_inits: Vec<_> = def
                .injected_fields
                .iter()
                .map(|f| {
                    let n = &f.name;
                    quote! { #n: __state.#n.clone() }
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
                            let __cfg = <quarlus_core::QuarlusConfig as axum::extract::FromRef<#state_type>>::from_ref(&__state);
                            __cfg.get(#key).unwrap_or_else(|e| panic!("Config key '{}' error: {}", #key, e))
                        }
                    }
                })
                .collect();

            let all_inits: Vec<_> = inject_inits
                .iter()
                .chain(config_inits.iter())
                .cloned()
                .collect();

            quote! {
                {
                    let __event_bus = state.#bus_field.clone();
                    let __state = state.clone();
                    __event_bus.subscribe(move |__event: std::sync::Arc<#event_type>| {
                        let __state = __state.clone();
                        async move {
                            let __ctrl = #controller_name {
                                #(#all_inits,)*
                            };
                            __ctrl.#fn_name(__event).await;
                        }
                    }).await;
                }
            }
        })
        .collect();

    let register_consumers_fn = if consumer_registrations.is_empty() {
        quote! {}
    } else {
        quote! {
            fn register_consumers(
                state: #state_type,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    #(#consumer_registrations)*
                })
            }
        }
    };

    let router_body = if let Some(ref prefix) = def.prefix {
        quote! {
            axum::Router::new()
                .nest(#prefix, axum::Router::new() #(#route_registrations)*)
        }
    } else {
        quote! {
            axum::Router::new()
                #(#route_registrations)*
        }
    };

    quote! {
        impl quarlus_core::Controller<#state_type> for #name {
            fn routes() -> axum::Router<#state_type> {
                #router_body
            }

            fn route_metadata() -> Vec<quarlus_core::openapi::RouteInfo> {
                vec![#(#route_metadata_items),*]
            }

            #register_consumers_fn
        }
    }
}

/// Extract the request body type name and inner type from a method parameter.
/// Detects `Json<T>` and `Validated<T>` extractors (possibly path-qualified like `axum::Json<T>`).
/// Returns `(type_name, inner_syn_type)` for schema generation.
fn extract_body_type_info(ty: &syn::Type) -> Option<(String, syn::Type)> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if ident == "Json" || ident == "Validated" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                        if let syn::Type::Path(inner_path) = inner_ty {
                            if let Some(inner_seg) = inner_path.path.segments.last() {
                                return Some((inner_seg.ident.to_string(), inner_ty.clone()));
                            }
                        }
                    }
                }
            }
        }
    }
    None
}
