//! Controller trait implementation generation.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::routes_parsing::RoutesImplDef;

/// Generate the `Controller<State>` trait implementation.
pub fn generate_controller_impl(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;
    let meta_mod = format_ident!("__quarlus_meta_{}", name);

    let route_registrations = generate_route_registrations(def, name);
    let route_metadata_items = generate_route_metadata(def, name, &meta_mod);
    let register_consumers_fn = generate_consumer_registrations(def, name, &meta_mod);
    let scheduled_tasks_fn = generate_scheduled_tasks(def, name, &meta_mod);

    let router_body = quote! {
        {
            let __inner = quarlus_core::http::Router::new()
                #(#route_registrations)*;
            match #meta_mod::PATH_PREFIX {
                Some("/") | None => __inner,
                Some(__prefix) => quarlus_core::http::Router::new().nest(__prefix, __inner),
            }
        }
    };

    quote! {
        impl quarlus_core::Controller<#meta_mod::State> for #name {
            fn routes() -> quarlus_core::http::Router<#meta_mod::State> {
                #router_body
            }

            fn route_metadata() -> Vec<quarlus_core::openapi::RouteInfo> {
                vec![#(#route_metadata_items),*]
            }

            #register_consumers_fn

            #scheduled_tasks_fn
        }
    }
}

/// Generate route registration expressions for the router.
fn generate_route_registrations(def: &RoutesImplDef, name: &syn::Ident) -> Vec<TokenStream> {
    def.route_methods
        .iter()
        .map(|rm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, rm.fn_item.sig.ident);
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());

            let has_layers = !rm.middleware_fns.is_empty() || !rm.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, quarlus_core::http::routing::#method_fn(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = rm
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| {
                        quote! { .layer(quarlus_core::http::middleware::from_fn(#mw_fn)) }
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
                        quarlus_core::http::routing::#method_fn(#handler_name)
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
    let tag_name = name.to_string();

    def.route_methods
        .iter()
        .map(|rm| {
            let route_path_str = &rm.path;
            let method = rm.method.as_routing_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let params = extract_path_params(rm);
            let (body_type_token, body_schema_token) = extract_body_info(rm);

            quote! {
                quarlus_core::openapi::RouteInfo {
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
fn extract_path_params(rm: &crate::types::RouteMethod) -> Vec<TokenStream> {
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
                            let __ctrl = <#controller_name as quarlus_core::StatefulConstruct<#meta_mod::State>>::from_state(&__state);
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
fn generate_scheduled_tasks(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> TokenStream {
    if def.scheduled_methods.is_empty() {
        return quote! {};
    }

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

            let schedule_expr = generate_schedule_expr(sm);

            let is_async = sm.fn_item.sig.asyncness.is_some();
            let call_expr = if is_async {
                quote! { __ctrl.#fn_name().await }
            } else {
                quote! { __ctrl.#fn_name() }
            };

            quote! {
                quarlus_core::ScheduledTaskDef {
                    name: #task_name.to_string(),
                    schedule: #schedule_expr,
                    task: Box::new(move |__state: #meta_mod::State| {
                        Box::pin(async move {
                            let __ctrl = <#name as quarlus_core::StatefulConstruct<#meta_mod::State>>::from_state(&__state);
                            quarlus_core::ScheduledResult::log_if_err(
                                #call_expr,
                                #task_name,
                            );
                        })
                    }),
                }
            }
        })
        .collect();

    quote! {
        fn scheduled_tasks() -> Vec<quarlus_core::ScheduledTaskDef<#meta_mod::State>> {
            vec![#(#task_defs),*]
        }
    }
}

/// Generate schedule configuration expression.
fn generate_schedule_expr(sm: &crate::types::ScheduledMethod) -> TokenStream {
    if let Some(every) = sm.config.every {
        if let Some(delay) = sm.config.initial_delay {
            quote! {
                quarlus_core::ScheduleConfig::IntervalWithDelay {
                    interval: std::time::Duration::from_secs(#every),
                    initial_delay: std::time::Duration::from_secs(#delay),
                }
            }
        } else {
            quote! {
                quarlus_core::ScheduleConfig::Interval(
                    std::time::Duration::from_secs(#every)
                )
            }
        }
    } else {
        let cron_expr = sm.config.cron.as_ref().unwrap();
        quote! {
            quarlus_core::ScheduleConfig::Cron(#cron_expr.to_string())
        }
    }
}
