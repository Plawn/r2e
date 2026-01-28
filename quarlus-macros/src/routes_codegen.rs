use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::routes_parsing::RoutesImplDef;
use crate::types::*;

pub fn generate(def: &RoutesImplDef) -> TokenStream {
    let impl_block = generate_impl_block(def);
    let handlers = generate_handlers(def);
    let controller_impl = generate_controller_impl(def);
    let scheduled_impl = generate_scheduled_impl(def);

    quote! {
        #impl_block
        #handlers
        #controller_impl
        #scheduled_impl
    }
}

// ---------------------------------------------------------------------------
// Impl block with wrapped methods
// ---------------------------------------------------------------------------

fn generate_impl_block(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;

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

    if route_fns.is_empty()
        && consumer_fns.is_empty()
        && scheduled_fns.is_empty()
        && other_fns.is_empty()
    {
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
// Method wrapping (interceptors + transactional)
// ---------------------------------------------------------------------------

fn generate_wrapped_method(rm: &RouteMethod, def: &RoutesImplDef) -> TokenStream {
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
    let controller_name_str = def.controller_name.to_string();
    let original_body = &fn_item.block;

    let mut body: TokenStream = quote! { #original_body };

    // Inline wrapper (innermost): transactional
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

    // Trait-based interceptors (via Interceptor::around)
    let all_intercepts: Vec<&syn::Expr> = def
        .controller_intercepts
        .iter()
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

fn generate_wrapped_scheduled_method(sm: &ScheduledMethod, def: &RoutesImplDef) -> TokenStream {
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
    let controller_name_str = def.controller_name.to_string();
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
// Handler generation
// ---------------------------------------------------------------------------

fn generate_handlers(def: &RoutesImplDef) -> TokenStream {
    let handlers: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| generate_single_handler(def, rm))
        .collect();

    quote! { #(#handlers)* }
}

fn generate_single_handler(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let controller_name = &def.controller_name;
    let meta_mod = format_ident!("__quarlus_meta_{}", controller_name);
    let extractor_name = format_ident!("__QuarlusExtract_{}", controller_name);
    let fn_name = &rm.fn_item.sig.ident;
    let handler_name = format_ident!("__quarlus_{}_{}", controller_name, fn_name);
    let return_type = &rm.fn_item.sig.output;
    let fn_name_str = fn_name.to_string();
    let controller_name_str = controller_name.to_string();

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

    let call_expr = if rm.fn_item.sig.asyncness.is_some() {
        quote! { __ctrl.#fn_name(#(#call_args),*).await }
    } else {
        quote! { __ctrl.#fn_name(#(#call_args),*) }
    };

    let needs_response = !rm.guard_fns.is_empty();

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
        // Guarded handler: returns Response to allow short-circuit
        let guard_checks: Vec<TokenStream> = rm
            .guard_fns
            .iter()
            .map(|guard_expr| {
                quote! {
                    if let Err(__resp) = quarlus_core::Guard::check(
                        &#guard_expr,
                        &__state,
                        &__guard_ctx,
                    ) {
                        return __resp;
                    }
                }
            })
            .collect();

        // Build guard context based on identity source
        let guard_context_construction = if let Some(ref id_param) = rm.identity_param {
            // Case A: param-level identity — use the handler param as identity source
            let arg_name = format_ident!("__arg_{}", id_param.index);
            quote! {
                let __guard_ctx = quarlus_core::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    identity: Some(&#arg_name),
                };
            }
        } else {
            // Case B: struct-level identity or no identity
            quote! {
                let __guard_ctx = quarlus_core::GuardContext {
                    method_name: #fn_name_str,
                    controller_name: #controller_name_str,
                    headers: &__headers,
                    identity: #meta_mod::guard_identity(&__ctrl_ext.0),
                };
            }
        };

        quote! {
            #[allow(non_snake_case)]
            async fn #handler_name(
                axum::extract::State(__state): axum::extract::State<#meta_mod::State>,
                __headers: axum::http::HeaderMap,
                __ctrl_ext: #extractor_name,
                #(#handler_extra_params,)*
            ) -> axum::response::Response {
                #guard_context_construction
                #(#guard_checks)*
                let __ctrl = __ctrl_ext.0;
                axum::response::IntoResponse::into_response(#call_expr)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Controller trait impl
// ---------------------------------------------------------------------------

fn generate_controller_impl(def: &RoutesImplDef) -> TokenStream {
    let name = &def.controller_name;
    let meta_mod = format_ident!("__quarlus_meta_{}", name);

    let route_registrations: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| {
            let handler_name = format_ident!("__quarlus_{}_{}", name, rm.fn_item.sig.ident);
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_axum_method_fn());

            let has_layers = !rm.middleware_fns.is_empty() || !rm.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, axum::routing::#method_fn(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = rm
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| {
                        quote! { .layer(axum::middleware::from_fn(#mw_fn)) }
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
                        axum::routing::#method_fn(#handler_name)
                            #(#middleware_layers)*
                            #(#direct_layers)*
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
            let route_path_str = &rm.path;
            let method = rm.method.as_axum_method_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let params: Vec<_> = rm
                .fn_item
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
                .collect();

            let body_info: Option<(String, syn::Type)> =
                rm.fn_item.sig.inputs.iter().find_map(|arg| {
                    if let syn::FnArg::Typed(pt) = arg {
                        extract_body_type_info(&pt.ty)
                    } else {
                        None
                    }
                });
            let (body_type_token, body_schema_token) = match &body_info {
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
            };

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
        .collect();

    // --- Consumer registrations ---
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

    let register_consumers_fn = if consumer_registrations.is_empty() {
        quote! {}
    } else {
        quote! {
            fn register_consumers(
                __state: #meta_mod::State,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    #(#consumer_registrations)*
                })
            }
        }
    };

    let router_body = quote! {
        {
            let __inner = axum::Router::new()
                #(#route_registrations)*;
            match #meta_mod::PATH_PREFIX {
                Some(__prefix) => axum::Router::new().nest(__prefix, __inner),
                None => __inner,
            }
        }
    };

    quote! {
        impl quarlus_core::Controller<#meta_mod::State> for #name {
            fn routes() -> axum::Router<#meta_mod::State> {
                #router_body
            }

            fn route_metadata() -> Vec<quarlus_core::openapi::RouteInfo> {
                vec![#(#route_metadata_items),*]
            }

            #register_consumers_fn
        }
    }
}

// ---------------------------------------------------------------------------
// ScheduledController impl
// ---------------------------------------------------------------------------

fn generate_scheduled_impl(def: &RoutesImplDef) -> TokenStream {
    if def.scheduled_methods.is_empty() {
        return quote! {};
    }

    let name = &def.controller_name;
    let meta_mod = format_ident!("__quarlus_meta_{}", name);
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
                    task: Box::new(move |__state: #meta_mod::State| {
                        Box::pin(async move {
                            let __ctrl = <#name as quarlus_core::StatefulConstruct<#meta_mod::State>>::from_state(&__state);
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
        impl quarlus_scheduler::ScheduledController<#meta_mod::State> for #name {
            fn register_scheduled_tasks(
                __scheduler: &mut quarlus_scheduler::Scheduler<#meta_mod::State>,
            ) {
                #(#task_registrations)*
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
