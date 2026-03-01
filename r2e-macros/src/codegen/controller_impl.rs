//! Controller trait implementation generation.

use proc_macro2::TokenStream;

use quote::{format_ident, quote};

use crate::crate_path::{r2e_core_path, r2e_events_path, r2e_scheduler_path};
use crate::routes_parsing::RoutesImplDef;

/// Generate the `Controller<State>` trait implementation.
pub fn generate_controller_impl(def: &RoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.controller_name;
    let meta_mod = format_ident!("__r2e_meta_{}", name);

    let route_registrations = generate_route_registrations(def, name);
    let sse_route_registrations = generate_sse_route_registrations(def, name);
    let ws_route_registrations = generate_ws_route_registrations(def, name);
    let route_metadata_items = generate_route_metadata(def, name, &meta_mod);
    let sse_metadata_items = generate_sse_route_metadata(def, name, &meta_mod);
    let ws_metadata_items = generate_ws_route_metadata(def, name, &meta_mod);
    let register_consumers_fn = generate_consumer_registrations(def, name, &meta_mod);
    let scheduled_tasks_fn = generate_scheduled_tasks(def, name, &meta_mod);
    let pre_auth_guards_fn = generate_pre_auth_guards(def, name, &meta_mod);

    // Only emit extend() calls for non-empty metadata lists to avoid
    // type inference issues with empty vec![].
    let register_meta_stmts = {
        let mut stmts = Vec::new();
        if !route_metadata_items.is_empty() {
            stmts.push(quote! { __registry.extend(vec![#(#route_metadata_items),*]); });
        }
        if !sse_metadata_items.is_empty() {
            stmts.push(quote! { __registry.extend(vec![#(#sse_metadata_items),*]); });
        }
        if !ws_metadata_items.is_empty() {
            stmts.push(quote! { __registry.extend(vec![#(#ws_metadata_items),*]); });
        }
        stmts
    };

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

            fn register_meta(__registry: &mut #krate::meta::MetaRegistry) {
                #(#register_meta_stmts)*
            }

            #pre_auth_guards_fn

            #register_consumers_fn

            #scheduled_tasks_fn

            fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                #meta_mod::validate_config(__config)
            }
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
        .any(|rm| !rm.decorators.pre_auth_guard_fns.is_empty());

    if !has_pre_auth {
        return quote! {};
    }

    let krate = r2e_core_path();
    let controller_name_str = name.to_string();

    // Collect pre-auth guard checks grouped by route path+method.
    // We generate route-specific sub-routers with the pre-auth middleware.
    let pre_auth_routes: Vec<TokenStream> = def
        .route_methods
        .iter()
        .filter(|rm| !rm.decorators.pre_auth_guard_fns.is_empty())
        .map(|rm| {
            let handler_name = format_ident!("__r2e_{}_{}", name, rm.fn_item.sig.ident);
            let fn_name_str = rm.fn_item.sig.ident.to_string();
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());
            let controller_name_str = &controller_name_str;

            let pre_auth_checks: Vec<_> = rm
                .decorators
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
                .decorators
                .middleware_fns
                .iter()
                .map(|mw_fn| {
                    quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) }
                })
                .collect();

            let direct_layers: Vec<_> = rm
                .decorators
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
    let krate = r2e_core_path();

    def.route_methods
        .iter()
        .filter(|rm| rm.decorators.pre_auth_guard_fns.is_empty())
        .map(|rm| {
            let handler_name = format_ident!("__r2e_{}_{}", name, rm.fn_item.sig.ident);
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());

            let has_layers = !rm.decorators.middleware_fns.is_empty() || !rm.decorators.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::#method_fn(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = rm
                    .decorators
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| {
                        quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) }
                    })
                    .collect();

                let direct_layers: Vec<_> = rm
                    .decorators
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
    let krate = r2e_core_path();
    let tag_name = name.to_string();

    def.route_methods
        .iter()
        .map(|rm| {
            let route_path_str = &rm.path;
            let method = rm.method.as_routing_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.decorators.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let path_params = extract_path_params(rm, &krate);
            let handler_param_types = extract_handler_param_types(rm);
            let (body_type_token, body_schema_token) = extract_body_info(rm);
            let (response_type_token, response_schema_token) = extract_response_info(rm);

            // Extract doc comments for summary + description
            let (doc_summary, doc_description) =
                crate::extract::route::extract_doc_comments(&rm.fn_item.attrs);
            let summary_token = match doc_summary {
                Some(s) => quote! { Some(#s.to_string()) },
                None => quote! { None },
            };
            let description_token = match doc_description {
                Some(d) => quote! { Some(#d.to_string()) },
                None => quote! { None },
            };

            // Status: #[status(N)] override > default_status_for_method
            let status_code = rm.decorators.status_override
                .unwrap_or_else(|| default_status_for_method(&rm.method));

            let deprecated = rm.decorators.deprecated;
            let body_required = detect_body_required(rm);

            // has_auth: roles, identity param, guard fns, or struct-level identity
            let has_roles = !rm.decorators.roles.is_empty();
            let has_identity_param = rm.identity_param.is_some();
            let has_guards = !rm.decorators.guard_fns.is_empty();

            // Autoref specialization: for each handler param type, probe for ParamsMetadata.
            // Types implementing ParamsMetadata return their param infos; others return empty vec.
            let probe_blocks: Vec<TokenStream> = handler_param_types
                .iter()
                .map(|ty| {
                    quote! {
                        {
                            let __probe = #krate::params::__ParamMetaProbe::<#ty>(::core::marker::PhantomData);
                            use #krate::params::__NoParamsMeta as _;
                            __p.extend((&__probe).param_infos());
                        }
                    }
                })
                .collect();

            quote! {
                #krate::meta::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #route_path_str),
                        None => #route_path_str.to_string(),
                    },
                    method: #method.to_string(),
                    operation_id: #op_id.to_string(),
                    summary: #summary_token,
                    description: #description_token,
                    request_body_type: #body_type_token,
                    request_body_schema: #body_schema_token,
                    request_body_required: #body_required,
                    response_type: #response_type_token,
                    response_schema: #response_schema_token,
                    response_status: #status_code,
                    params: {
                        let mut __p: Vec<#krate::meta::ParamInfo> = vec![#(#path_params),*];
                        #(#probe_blocks)*
                        // Deduplicate params by (name, location) — possible when
                        // a Params struct includes #[path] fields alongside Path<T>.
                        {
                            let mut seen = ::std::collections::HashSet::new();
                            __p.retain(|p| seen.insert((p.name.clone(), format!("{:?}", p.location))));
                        }
                        __p
                    },
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                    deprecated: #deprecated,
                    has_auth: #has_roles || #has_identity_param || #has_guards || #meta_mod::HAS_STRUCT_IDENTITY,
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
                let ty = &pt.ty;
                let ty_str = quote!(#ty).to_string();
                if ty_str.contains("Path") {
                    if let syn::Pat::TupleStruct(ts) = pt.pat.as_ref() {
                        if let Some(elem) = ts.elems.first() {
                            let param_name = quote!(#elem).to_string();
                            let param_type = infer_path_param_openapi_type(&pt.ty);
                            return Some(quote! {
                                #krate::meta::ParamInfo {
                                    name: #param_name.to_string(),
                                    location: #krate::meta::ParamLocation::Path,
                                    param_type: #param_type.to_string(),
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

/// Extract the types to probe for `ParamsMetadata` from handler parameters.
///
/// For wrapper types like `Query<T>`, `Path<T>`, we unwrap to probe the inner
/// type `T` instead, since `T` is where `ParamsMetadata` would be implemented.
fn extract_handler_param_types(rm: &crate::types::RouteMethod) -> Vec<syn::Type> {
    rm.fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let syn::FnArg::Typed(pt) = arg {
                Some(unwrap_extractor_inner(&pt.ty))
            } else {
                None // skip &self
            }
        })
        .collect()
}

/// Unwrap generic wrapper types to get the inner type for metadata probing.
/// `Query<T>` → `T`, `Path<T>` → `T`, other types → unchanged.
fn unwrap_extractor_inner(ty: &syn::Type) -> syn::Type {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident_str = segment.ident.to_string();
            if matches!(ident_str.as_str(), "Query" | "Path") {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return inner.clone();
                    }
                }
            }
        }
    }
    ty.clone()
}

/// Infer an OpenAPI type string from a `Path<T>` type.
/// Returns "integer" for integer types, "number" for floats, "boolean" for bool, otherwise "string".
fn infer_path_param_openapi_type(ty: &syn::Type) -> &'static str {
    // Extract the inner type from Path<T>
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Path" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return type_to_openapi_str(inner);
                    }
                }
            }
        }
    }
    "string"
}

/// Map a syn::Type to an OpenAPI type string by inspecting the last path segment.
fn type_to_openapi_str(ty: &syn::Type) -> &'static str {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return match segment.ident.to_string().as_str() {
                "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64"
                | "isize" => "integer",
                "f32" | "f64" => "number",
                "bool" => "boolean",
                _ => "string",
            };
        }
    }
    "string"
}

/// Determine the default HTTP status code based on the HTTP method.
fn default_status_for_method(method: &crate::route::HttpMethod) -> u16 {
    match method {
        crate::route::HttpMethod::Get => 200,
        crate::route::HttpMethod::Post => 201,
        crate::route::HttpMethod::Put => 200,
        crate::route::HttpMethod::Delete => 204,
        crate::route::HttpMethod::Patch => 200,
    }
}

/// Check if the body parameter is `Option<Json<T>>` → required: false, `Json<T>` → required: true.
fn detect_body_required(rm: &crate::types::RouteMethod) -> bool {
    for arg in rm.fn_item.sig.inputs.iter() {
        if let syn::FnArg::Typed(pt) = arg {
            if has_json_type(&pt.ty) {
                // Check if it's wrapped in Option
                if is_option_wrapping_json(&pt.ty) {
                    return false;
                }
                return true;
            }
        }
    }
    true
}

/// Check if a type is `Option<Json<T>>`.
fn is_option_wrapping_json(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return has_json_type(inner);
                    }
                }
            }
        }
    }
    false
}

/// Check if a type contains Json (is `Json<T>` or a destructured pattern).
fn has_json_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Json";
        }
    }
    false
}

/// Unwrap `Result<T, E>` → `T`, leaving non-Result types unchanged.
fn unwrap_result_type(ty: &syn::Type) -> &syn::Type {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident_str = segment.ident.to_string();
            if ident_str == "Result" || ident_str == "ApiResult" || ident_str == "JsonResult" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return inner;
                    }
                }
            }
        }
    }
    ty
}

/// Extract the inner type from `Json<T>` → `T`.
fn unwrap_json_type(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Json" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Check if a type is a "no body" type (StatusCode, StatusResult, ()).
fn is_no_body_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident_str = segment.ident.to_string();
            return matches!(ident_str.as_str(), "StatusCode" | "StatusResult");
        }
    }
    if let syn::Type::Tuple(tuple) = ty {
        return tuple.elems.is_empty(); // ()
    }
    false
}

/// Convert a syn::Type to an OpenAPI-friendly name string.
fn type_to_schema_name(ty: &syn::Type) -> String {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                let inner_names: Vec<String> = args
                    .args
                    .iter()
                    .filter_map(|arg| {
                        if let syn::GenericArgument::Type(inner_ty) = arg {
                            Some(type_to_schema_name(inner_ty))
                        } else {
                            None
                        }
                    })
                    .collect();
                if !inner_names.is_empty() {
                    return format!("{}_{}", ident, inner_names.join("_"));
                }
            }
            return ident;
        }
    }
    quote!(#ty).to_string().replace(' ', "")
}

/// Generate a schema token for a response type, using autoref specialization
/// so that types not implementing `JsonSchema` gracefully return `None`.
fn response_schema_token(ty: &syn::Type) -> TokenStream {
    if let Some(schemars) = crate::crate_path::r2e_schemars_path() {
        quote! {
            {
                // Autoref specialization: if #ty implements JsonSchema, this
                // resolves to the inherent method returning Some(schema).
                // Otherwise, the trait fallback returns None.
                struct __RespProbe<T>(::core::marker::PhantomData<T>);
                trait __NoRespSchema {
                    fn __resp_schema(&self) -> Option<serde_json::Value> { None }
                }
                impl<T> __NoRespSchema for &__RespProbe<T> {}
                impl<T: #schemars::JsonSchema> __RespProbe<T> {
                    fn __resp_schema(&self) -> Option<serde_json::Value> {
                        Some(serde_json::to_value(#schemars::schema_for!(T)).unwrap())
                    }
                }
                let __p = __RespProbe::<#ty>(::core::marker::PhantomData);
                use __NoRespSchema as _;
                (&__p).__resp_schema()
            }
        }
    } else {
        quote! { None }
    }
}

/// Resolve the inner response type from the route method.
/// Returns `Some(ty)` if a JSON response body type is detected, `None` otherwise.
fn resolve_response_type(rm: &crate::types::RouteMethod) -> Option<syn::Type> {
    // #[returns(T)] override takes priority
    if let Some(ref returns_ty) = rm.decorators.returns_type {
        return Some(returns_ty.clone());
    }

    // Analyze return type
    let output = &rm.fn_item.sig.output;
    let ret_ty = match output {
        syn::ReturnType::Default => return None,
        syn::ReturnType::Type(_, ty) => ty.as_ref(),
    };

    // impl Trait → no detection
    if matches!(ret_ty, syn::Type::ImplTrait(_)) {
        return None;
    }

    // Unwrap Result/ApiResult/JsonResult
    let unwrapped = unwrap_result_type(ret_ty);

    // Check for no-body types
    if is_no_body_type(unwrapped) {
        return None;
    }

    // Check for String — no schema
    if let syn::Type::Path(type_path) = unwrapped {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "String" {
                return None;
            }
        }
    }

    // Try to unwrap Json<T>
    unwrap_json_type(unwrapped).cloned()
}

/// Extract response type information from a route method.
/// Returns (response_type_name_token, response_schema_token).
fn extract_response_info(rm: &crate::types::RouteMethod) -> (TokenStream, TokenStream) {
    match resolve_response_type(rm) {
        Some(inner_ty) => {
            let name = type_to_schema_name(&inner_ty);
            let schema_token = response_schema_token(&inner_ty);
            (quote! { Some(#name.to_string()) }, schema_token)
        }
        None => (quote! { None }, quote! { None }),
    }
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
        Some((bname, inner_ty)) => {
            let schema_token = if let Some(schemars) = crate::crate_path::r2e_schemars_path() {
                quote! {
                    Some({
                        let __schema = #schemars::schema_for!(#inner_ty);
                        serde_json::to_value(__schema).unwrap()
                    })
                }
            } else {
                quote! { None }
            };
            (quote! { Some(#bname.to_string()) }, schema_token)
        }
        None => (quote! { None }, quote! { None }),
    }
}

/// Extract body type info from Json<T> types.
fn extract_body_type_info(ty: &syn::Type) -> Option<(String, syn::Type)> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if ident == "Json" {
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

    let krate = r2e_core_path();
    let events_krate = r2e_events_path();
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
                    #events_krate::EventBus::subscribe(&__event_bus, move |__event: std::sync::Arc<#event_type>| {
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
/// `r2e-scheduler`, not `r2e-core`. The macro generates code referencing the
/// scheduler crate. If users use `#[scheduled]`, they need `r2e-scheduler` as a dep.
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

    let krate = r2e_core_path();
    let sched_krate = r2e_scheduler_path();
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
    let krate = r2e_core_path();

    def.sse_methods
        .iter()
        .filter(|sm| sm.decorators.pre_auth_guard_fns.is_empty())
        .map(|sm| {
            let handler_name = format_ident!("__r2e_{}_{}", name, sm.fn_item.sig.ident);
            let path = &sm.path;

            let has_layers = !sm.decorators.middleware_fns.is_empty() || !sm.decorators.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::get(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = sm
                    .decorators
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
                    .collect();
                let direct_layers: Vec<_> = sm
                    .decorators
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
    let krate = r2e_core_path();
    let tag_name = name.to_string();

    def.sse_methods
        .iter()
        .map(|sm| {
            let path = &sm.path;
            let op_id = format!("{}_{}", name, sm.fn_item.sig.ident);
            let roles: Vec<_> = sm.decorators.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let has_roles = !sm.decorators.roles.is_empty();
            let has_guards = !sm.decorators.guard_fns.is_empty();
            let has_identity_param = sm.identity_param.is_some();

            quote! {
                #krate::meta::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #path),
                        None => #path.to_string(),
                    },
                    method: "GET".to_string(),
                    operation_id: #op_id.to_string(),
                    summary: Some("SSE stream".to_string()),
                    description: None,
                    request_body_type: None,
                    request_body_schema: None,
                    request_body_required: true,
                    response_type: None,
                    response_schema: None,
                    response_status: 200,
                    params: vec![],
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                    deprecated: false,
                    has_auth: #has_roles || #has_identity_param || #has_guards || #meta_mod::HAS_STRUCT_IDENTITY,
                }
            }
        })
        .collect()
}

// ── WS route registration ───────────────────────────────────────────────

fn generate_ws_route_registrations(def: &RoutesImplDef, name: &syn::Ident) -> Vec<TokenStream> {
    let krate = r2e_core_path();

    def.ws_methods
        .iter()
        .filter(|wm| wm.decorators.pre_auth_guard_fns.is_empty())
        .map(|wm| {
            let handler_name = format_ident!("__r2e_{}_{}", name, wm.fn_item.sig.ident);
            let path = &wm.path;

            let has_layers = !wm.decorators.middleware_fns.is_empty() || !wm.decorators.layer_exprs.is_empty();

            if !has_layers {
                quote! {
                    .route(#path, #krate::http::routing::get(#handler_name))
                }
            } else {
                let middleware_layers: Vec<_> = wm
                    .decorators
                    .middleware_fns
                    .iter()
                    .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
                    .collect();
                let direct_layers: Vec<_> = wm
                    .decorators
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
    let krate = r2e_core_path();
    let tag_name = name.to_string();

    def.ws_methods
        .iter()
        .map(|wm| {
            let path = &wm.path;
            let op_id = format!("{}_{}", name, wm.fn_item.sig.ident);
            let roles: Vec<_> = wm.decorators.roles.iter().map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let has_roles = !wm.decorators.roles.is_empty();
            let has_guards = !wm.decorators.guard_fns.is_empty();
            let has_identity_param = wm.identity_param.is_some();

            quote! {
                #krate::meta::RouteInfo {
                    path: match #meta_mod::PATH_PREFIX {
                        Some(__prefix) => format!("{}{}", __prefix, #path),
                        None => #path.to_string(),
                    },
                    method: "GET".to_string(),
                    operation_id: #op_id.to_string(),
                    summary: Some("WebSocket endpoint".to_string()),
                    description: None,
                    request_body_type: None,
                    request_body_schema: None,
                    request_body_required: true,
                    response_type: None,
                    response_schema: None,
                    response_status: 200,
                    params: vec![],
                    roles: vec![#(#roles),*],
                    tag: Some(#tag.to_string()),
                    deprecated: false,
                    has_auth: #has_roles || #has_identity_param || #has_guards || #meta_mod::HAS_STRUCT_IDENTITY,
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
