//! Controller trait implementation generation.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::crate_path::{quarlus_core_path, quarlus_scheduler_path};
use crate::routes_parsing::RoutesImplDef;

/// Generate the `Controller<State>` trait implementation.
pub fn generate_controller_impl(def: &RoutesImplDef) -> TokenStream {
    let krate = quarlus_core_path();
    let name = &def.controller_name;
    let meta_mod = format_ident!("__quarlus_meta_{}", name);

    let route_registrations = generate_route_registrations(def, name);
    let sse_route_registrations = generate_sse_route_registrations(def, name);
    let ws_route_registrations = generate_ws_route_registrations(def, name);
    let route_metadata_items = generate_route_metadata(def, name, &meta_mod);
    let sse_metadata_items = generate_sse_route_metadata(def, name, &meta_mod);
    let ws_metadata_items = generate_ws_route_metadata(def, name, &meta_mod);
    let register_consumers_fn = generate_consumer_registrations(def, name, &meta_mod);
    let scheduled_tasks_fn = generate_scheduled_tasks(def, name, &meta_mod);
    let pre_auth_guards_fn = generate_pre_auth_guards(def, name, &meta_mod);

    let router_body = quote! {
        {
            let __inner = #krate::http::Router::new()
                #(#route_registrations)*
                #(#sse_route_registrations)*
                #(#ws_route_registrations)*;
            match #meta_mod::PATH_PREFIX {
                Some("/") | None => __inner,
                Some(__prefix) => #krate::http::Router::new().nest(__prefix, __inner),
            }
        }
    };

    quote! {
        impl #krate::Controller<#meta_mod::State> for #name {
            fn routes() -> #krate::http::Router<#meta_mod::State> {
                #router_body
            }

            fn route_metadata() -> Vec<#krate::openapi::RouteInfo> {
                let mut __meta = vec![#(#route_metadata_items),*];
                __meta.extend(vec![#(#sse_metadata_items),*]);
                __meta.extend(vec![#(#ws_metadata_items),*]);
                __meta
            }

            #pre_auth_guards_fn

            #register_consumers_fn

            #scheduled_tasks_fn
        }
    }
}

/// Generate pre-auth guard middleware and `apply_pre_auth_guards` trait method.
///
/// Routes with pre-auth guards are registered in `routes()` as normal, but then
/// `apply_pre_auth_guards` wraps them with an additional middleware layer.
/// Since `apply_pre_auth_guards` receives a reference to the state, we can use
/// a state-capturing closure to access the rate-limit registry etc.
fn generate_pre_auth_guards(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> TokenStream {
    let has_pre_auth = def
        .route_methods
        .iter()
        .any(|rm| !rm.pre_auth_guard_fns.is_empty());

    if !has_pre_auth {
        return quote! {};
    }

    let krate = quarlus_core_path();
    let controller_name_str = name.to_string();

    // Collect pre-auth guard checks grouped by route path+method.
    // We generate route-specific sub-routers with the pre-auth middleware.
    let pre_auth_routes: Vec<TokenStream> = def
        .route_methods
        .iter()
        .filter(|rm| !rm.pre_auth_guard_fns.is_empty())
        .map(|rm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, rm.fn_item.sig.ident);
            let fn_name_str = rm.fn_item.sig.ident.to_string();
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());
            let controller_name_str = &controller_name_str;

            let pre_auth_checks: Vec<_> = rm
                .pre_auth_guard_fns
                .iter()
                .map(|guard_expr| {
                    quote! {
                        if let Err(__resp) = #krate::PreAuthGuard::check(
                            &#guard_expr,
                            &__mw_state,
                            &__pre_ctx,
                        ).await {
                            return __resp;
                        }
                    }
                })
                .collect();

            let middleware_layers: Vec<_> = rm
                .middleware_fns
                .iter()
                .map(|mw_fn| {
                    quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) }
                })
                .collect();

            let direct_layers: Vec<_> = rm
                .layer_exprs
                .iter()
                .map(|expr| {
                    quote! { .layer(#expr) }
                })
                .collect();

            quote! {
                {
                    let __state_for_mw = __state.clone();
                    let __pre_auth_mw = move |__req: #krate::http::extract::Request,
                                              __next: #krate::http::middleware::Next| {
                        let __mw_state = __state_for_mw.clone();
                        async move {
                            let __pre_ctx = #krate::PreAuthGuardContext {
                                method_name: #fn_name_str,
                                controller_name: #controller_name_str,
                                headers: __req.headers(),
                                uri: __req.uri(),
                                path_params: #krate::PathParams::EMPTY,
                            };
                            #(#pre_auth_checks)*
                            __next.run(__req).await
                        }
                    };
                    let __full_path = match #meta_mod::PATH_PREFIX {
                        Some("/") | None => #path.to_string(),
                        Some(__prefix) => format!("{}{}", __prefix, #path),
                    };
                    __router = __router.route(
                        &__full_path,
                        #krate::http::routing::#method_fn(#handler_name)
                            #(#middleware_layers)*
                            #(#direct_layers)*
                            .layer(#krate::http::middleware::from_fn(__pre_auth_mw))
                    );
                }
            }
        })
        .collect();

    quote! {
        fn apply_pre_auth_guards(
            mut __router: #krate::http::Router<#meta_mod::State>,
            __state: &#meta_mod::State,
        ) -> #krate::http::Router<#meta_mod::State> {
            #(#pre_auth_routes)*
            __router
        }
    }
}

/// Generate route registration expressions for the router.
/// Routes with pre-auth guards are excluded here and registered in `apply_pre_auth_guards` instead.
fn generate_route_registrations(def: &RoutesImplDef, name: &syn::Ident) -> Vec<TokenStream> {
    let krate = quarlus_core_path();

    def.route_methods
        .iter()
        .filter(|rm| rm.pre_auth_guard_fns.is_empty())
        .map(|rm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, rm.fn_item.sig.ident);
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());

            let has_layers = !rm.middleware_fns.is_empty() || !rm.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::#method_fn(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = rm
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| {
                        quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) }
                    })
                    .collect();

                let direct_layers: Vec<_> = rm
                    .layer_exprs
                    .iter()
                    .map(|expr| {
                        quote! { .layer(#expr) }
                    })
                    .collect();

                quote! {
                    .route(
                        #path,
                        #krate::http::routing::#method_fn(#handler_name)
                            #(#middleware_layers)*
                            #(#direct_layers)*
                    )
                }
            }
        })
        .collect()
}

/// Generate route metadata for OpenAPI documentation.
fn generate_route_metadata(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    let krate = quarlus_core_path();
    let tag_name = name.to_string();

    def.route_methods
        .iter()
        .map(|rm| {
            let route_path_str = &rm.path;
            let method = rm.method.as_routing_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let params = extract_path_params(rm, &krate);
            let (body_type_token, body_schema_token) = extract_body_info(rm);

            quote! {
                #krate::openapi::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #route_path_str),
                        None => #route_path_str.to_string(),
                    },
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
        .collect()
}

/// Extract path parameters from route method signature.
fn extract_path_params(rm: &crate::types::RouteMethod, krate: &TokenStream) -> Vec<TokenStream> {
    rm.fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let syn::FnArg::Typed(pt) = arg {
                let ty_str = quote!(#pt.ty).to_string();
                if ty_str.contains("Path") {
                    if let syn::Pat::TupleStruct(ts) = pt.pat.as_ref() {
                        if let Some(elem) = ts.elems.first() {
                            let param_name = quote!(#elem).to_string();
                            return Some(quote! {
                                #krate::openapi::ParamInfo {
                                    name: #param_name.to_string(),
                                    location: #krate::openapi::ParamLocation::Path,
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
        })
        .collect()
}

/// Extract request body type information.
fn extract_body_info(rm: &crate::types::RouteMethod) -> (TokenStream, TokenStream) {
    let body_info: Option<(String, syn::Type)> = rm.fn_item.sig.inputs.iter().find_map(|arg| {
        if let syn::FnArg::Typed(pt) = arg {
            extract_body_type_info(&pt.ty)
        } else {
            None
        }
    });

    match &body_info {
        Some((bname, inner_ty)) => (
            quote! { Some(#bname.to_string()) },
            quote! {
                Some({
                    let __schema = schemars::schema_for!(#inner_ty);
                    serde_json::to_value(__schema).unwrap()
                })
            },
        ),
        None => (quote! { None }, quote! { None }),
    }
}

/// Extract body type info from Json<T> or Validated<T> types.
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

/// Generate consumer registration function.
fn generate_consumer_registrations(
    def: &RoutesImplDef,
    _name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> TokenStream {
    if def.consumer_methods.is_empty() {
        return quote! {};
    }

    let krate = quarlus_core_path();
    let consumer_registrations: Vec<_> = def
        .consumer_methods
        .iter()
        .map(|cm| {
            let bus_field = format_ident!("{}", cm.bus_field);
            let event_type = &cm.event_type;
            let fn_name = &cm.fn_item.sig.ident;
            let controller_name = &def.controller_name;

            quote! {
                {
                    let __event_bus = __state.#bus_field.clone();
                    let __state = __state.clone();
                    __event_bus.subscribe(move |__event: std::sync::Arc<#event_type>| {
                        let __state = __state.clone();
                        async move {
                            let __ctrl = <#controller_name as #krate::StatefulConstruct<#meta_mod::State>>::from_state(&__state);
                            __ctrl.#fn_name(__event).await;
                        }
                    }).await;
                }
            }
        })
        .collect();

    quote! {
        fn register_consumers(
            __state: #meta_mod::State,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
            Box::pin(async move {
                #(#consumer_registrations)*
            })
        }
    }
}

/// Generate scheduled tasks function.
///
/// Note: Scheduling types (ScheduledTaskDef, ScheduleConfig, ScheduledResult, ScheduledTask) live in
/// `quarlus-scheduler`, not `quarlus-core`. The macro generates code referencing the
/// scheduler crate. If users use `#[scheduled]`, they need `quarlus-scheduler` as a dep.
///
/// Tasks capture the state at creation time. They are double-boxed:
/// 1. `Box<dyn ScheduledTask>` - the trait object
/// 2. `Box<dyn Any + Send>` - for type erasure in core
///
/// This allows the scheduler to downcast back to `Box<dyn ScheduledTask>` and call `start()`.
fn generate_scheduled_tasks(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> TokenStream {
    if def.scheduled_methods.is_empty() {
        return quote! {};
    }

    let krate = quarlus_core_path();
    let sched_krate = quarlus_scheduler_path();
    let controller_name_str = name.to_string();
    let task_defs: Vec<TokenStream> = def
        .scheduled_methods
        .iter()
        .map(|sm| {
            let fn_name = &sm.fn_item.sig.ident;
            let fn_name_str = fn_name.to_string();
            let task_name = match &sm.config.name {
                Some(n) => n.clone(),
                None => format!("{}_{}", controller_name_str, fn_name_str),
            };

            let schedule_expr = generate_schedule_expr(sm, &sched_krate);

            let is_async = sm.fn_item.sig.asyncness.is_some();
            let call_expr = if is_async {
                quote! { __ctrl.#fn_name().await }
            } else {
                quote! { __ctrl.#fn_name() }
            };

            // Task captures state via the `state` field
            quote! {
                {
                    let __task_def = #sched_krate::ScheduledTaskDef {
                        name: #task_name.to_string(),
                        schedule: #schedule_expr,
                        state: __state.clone(),
                        task: Box::new(move |__state: #meta_mod::State| {
                            Box::pin(async move {
                                let __ctrl = <#name as #krate::StatefulConstruct<#meta_mod::State>>::from_state(&__state);
                                #sched_krate::ScheduledResult::log_if_err(
                                    #call_expr,
                                    #task_name,
                                );
                            })
                        }),
                    };
                    // Double-box: first as trait object, then as Any for type erasure
                    let __boxed_task: Box<dyn #sched_krate::ScheduledTask> = Box::new(__task_def);
                    Box::new(__boxed_task) as Box<dyn std::any::Any + Send>
                }
            }
        })
        .collect();

    quote! {
        fn scheduled_tasks_boxed(__state: &#meta_mod::State) -> Vec<Box<dyn std::any::Any + Send>> {
            vec![#(#task_defs),*]
        }
    }
}

// ── SSE route registration ──────────────────────────────────────────────

fn generate_sse_route_registrations(def: &RoutesImplDef, name: &syn::Ident) -> Vec<TokenStream> {
    let krate = quarlus_core_path();

    def.sse_methods
        .iter()
        .filter(|sm| sm.pre_auth_guard_fns.is_empty())
        .map(|sm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, sm.fn_item.sig.ident);
            let path = &sm.path;

            let has_layers = !sm.middleware_fns.is_empty() || !sm.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::get(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = sm
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
                    .collect();
                let direct_layers: Vec<_> = sm
                    .layer_exprs
                    .iter()
                    .map(|expr| quote! { .layer(#expr) })
                    .collect();
                quote! {
                    .route(
                        #path,
                        #krate::http::routing::get(#handler_name)
                            #(#middleware_layers)*
                            #(#direct_layers)*
                    )
                }
            }
        })
        .collect()
}

fn generate_sse_route_metadata(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    let krate = quarlus_core_path();
    let tag_name = name.to_string();

    def.sse_methods
        .iter()
        .map(|sm| {
            let path = &sm.path;
            let op_id = format!("{}_{}", name, sm.fn_item.sig.ident);
            let roles: Vec<_> = sm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            quote! {
                #krate::openapi::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #path),
                        None => #path.to_string(),
                    },
                    method: "GET".to_string(),
                    operation_id: #op_id.to_string(),
                    summary: Some("SSE stream".to_string()),
                    request_body_type: None,
                    request_body_schema: None,
                    response_type: None,
                    params: vec![],
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                }
            }
        })
        .collect()
}

// ── WS route registration ───────────────────────────────────────────────

fn generate_ws_route_registrations(def: &RoutesImplDef, name: &syn::Ident) -> Vec<TokenStream> {
    let krate = quarlus_core_path();

    def.ws_methods
        .iter()
        .filter(|wm| wm.pre_auth_guard_fns.is_empty())
        .map(|wm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, wm.fn_item.sig.ident);
            let path = &wm.path;

            let has_layers = !wm.middleware_fns.is_empty() || !wm.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::get(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = wm
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
                    .collect();
                let direct_layers: Vec<_> = wm
                    .layer_exprs
                    .iter()
                    .map(|expr| quote! { .layer(#expr) })
                    .collect();
                quote! {
                    .route(
                        #path,
                        #krate::http::routing::get(#handler_name)
                            #(#middleware_layers)*
                            #(#direct_layers)*
                    )
                }
            }
        })
        .collect()
}

fn generate_ws_route_metadata(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    let krate = quarlus_core_path();
    let tag_name = name.to_string();

    def.ws_methods
        .iter()
        .map(|wm| {
            let path = &wm.path;
            let op_id = format!("{}_{}", name, wm.fn_item.sig.ident);
            let roles: Vec<_> = wm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            quote! {
                #krate::openapi::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #path),
                        None => #path.to_string(),
                    },
                    method: "GET".to_string(),
                    operation_id: #op_id.to_string(),
                    summary: Some("WebSocket endpoint".to_string()),
                    request_body_type: None,
                    request_body_schema: None,
                    response_type: None,
                    params: vec![],
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                }
            }
        })
        .collect()
}

/// Generate schedule configuration expression.
fn generate_schedule_expr(sm: &crate::types::ScheduledMethod, sched_krate: &TokenStream) -> TokenStream {
    if let Some(every) = sm.config.every {
        if let Some(delay) = sm.config.initial_delay {
            quote! {
                #sched_krate::ScheduleConfig::IntervalWithDelay {
                    interval: std::time::Duration::from_secs(#every),
                    initial_delay: std::time::Duration::from_secs(#delay),
                }
            }
        } else {
            quote! {
                #sched_krate::ScheduleConfig::Interval(
                    std::time::Duration::from_secs(#every)
                )
            }
        }
    } else {
        let cron_expr = sm.config.cron.as_ref().unwrap();
        quote! {
            #sched_krate::ScheduleConfig::Cron(#cron_expr.to_string())
        }
    }
}
