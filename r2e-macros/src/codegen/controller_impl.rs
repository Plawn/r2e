//! Controller trait implementation generation.

use proc_macro2::TokenStream;

use quote::{format_ident, quote};

use crate::codegen::transverse::{self, ConsumerMethodDef, DecoFieldDef, ScheduledSourceMethod};
use crate::crate_path::r2e_core_path;
use crate::routes_parsing::RoutesImplDef;
use crate::type_utils::type_last_segment_is;

/// Generate the `Controller<State>` trait implementation.
pub fn generate_controller_impl(def: &RoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let name = &def.controller_name;
    let meta_mod = format_ident!("__r2e_meta_{}", name);

    // Single registration path: each route captures the application core `Arc`
    // once at build time, then per request extracts `__R2eRequestData_<Name>`
    // and binds the façade. Consumers/scheduled tasks receive that same core;
    // a consumer/scheduled method touching a request-scoped field
    // fails to compile naturally because that field lives on the façade, not the
    // core impl.
    let route_registrations = generate_route_registrations(def);
    let sse_route_registrations = generate_sse_route_registrations(def);
    let ws_route_registrations = generate_ws_route_registrations(def);
    let pre_auth_registrations = generate_pre_auth_registrations(def, name, &meta_mod);
    // Controller deps = core `ContextConstruct::Deps` ++ every decorator
    // site's `<Spec as DecoratorSpec>::Deps`. Emitted once, on the
    // `EndpointDeps` carrier — checked by `AllSatisfied` at
    // `register_controller()` and by `ModuleDepsSatisfied` at
    // `register_module()`.
    let deps_fold = super::decorators::controller_deps_fold(def);
    let route_metadata_items = generate_route_metadata(def, name, &meta_mod);
    let sse_metadata_items = generate_sse_route_metadata(def, name, &meta_mod);
    let ws_metadata_items = generate_ws_route_metadata(def, name, &meta_mod);
    // Off-request (transverse) wiring: ScheduledSource / EventSubscriber /
    // BeanDecoFill / PostConstruct impls (module scope) + the Controller method
    // overrides that delegate to them.
    let (transverse_items, transverse_fns) = generate_transverse(def, name);

    let has_fallback = def.route_methods.iter().any(|rm| rm.is_fallback);

    // #[fallback] is app-wide (it handles every request no other route
    // matched), so it only makes sense on a controller mounted at the root.
    // PATH_PREFIX lives on the #[controller] side — enforce cross-macro with
    // a const assert on the meta module.
    let fallback_prefix_assert = if has_fallback {
        quote! {
            const _: () = {
                const fn __r2e_is_root_prefix(p: &str) -> bool {
                    let b = p.as_bytes();
                    b.is_empty() || (b.len() == 1 && b[0] == b'/')
                }
                match #meta_mod::PATH_PREFIX {
                    None => {}
                    Some(p) => assert!(
                        __r2e_is_root_prefix(p),
                        "#[fallback] requires a controller without a path prefix: the fallback \
                         handles every unmatched request app-wide, which a `path = \"...\"` \
                         prefix would not scope. Move it to a root-mounted controller."
                    ),
                }
            };
        }
    } else {
        quote! {}
    };

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

    // Controller-level (impl-level) `#[intercept]` products, built ONCE per
    // controller and shared (via `Arc` clones) across every HTTP route — so a
    // stateful impl-level interceptor keeps a single instance, not one per route.
    // Emitted only when there are controller-level interceptors AND at least one
    // HTTP route to capture them (SSE/WS run no interceptor chain).
    let ctrl_deco_items = super::decorators::generate_ctrl_deco_items(def);
    let ctrl_router_setup = super::decorators::ctrl_deco_set(def)
        .filter(|_| !def.route_methods.is_empty())
        .map(|set| {
            let ctor = &set.ctor_ident;
            quote! { let __r2e_ctrl_deco = ::std::sync::Arc::new(#ctor(__ctx)); }
        });

    // Application-scoped router body. The controller Arc is captured once at
    // router build time and reused per request; route decorator sets are
    // built here from the resolved bean context, once per route.
    let application_router_body = quote! {
        |__ctrl: ::std::sync::Arc<#name>, __ctx: &#krate::beans::BeanContext| {
            #ctrl_router_setup
            let mut __inner = #krate::http::Router::new()
                #(#route_registrations)*
                #(#sse_route_registrations)*
                #(#ws_route_registrations)*;
            #(#pre_auth_registrations)*
            match #meta_mod::PATH_PREFIX {
                Some("/") | None => __inner,
                Some(__prefix) => #krate::http::Router::new().nest(__prefix, __inner),
            }
        }
    };

    // ── State-generic impl assembly ─────────────────────────────────────
    //
    // The impl is generic over the state `__R2eS` plus one opaque marker per
    // extraction site: `__R2eMd` for the request-data struct (a tuple of
    // per-field markers, shape known only to `#[controller]`) and one
    // `__R2eMp_<fn>` per param-level `#[inject(identity)]`. The markers are
    // folded into the `Controller<S, W>` witness parameter so registration can
    // infer them (E0207 forbids leaving them unconstrained on the impl).
    let state_ident = super::handlers::state_generic();
    let md = super::handlers::data_marker();
    let data_name = format_ident!("__R2eRequestData_{}", name);
    let state_bounds = super::handlers::state_bounds(&krate);

    let mut param_markers: Vec<syn::Ident> = Vec::new();
    let mut param_marker_bounds: Vec<TokenStream> = Vec::new();
    {
        let mut push_identity = |fn_item: &syn::ImplItemFn, index: usize| {
            let marker = super::handlers::identity_marker_for(&fn_item.sig.ident);
            let declared_ty = fn_item
                .sig
                .inputs
                .iter()
                .filter_map(|arg| match arg {
                    syn::FnArg::Typed(pt) => Some(pt),
                    syn::FnArg::Receiver(_) => None,
                })
                .nth(index)
                .map(|pt| (*pt.ty).clone())
                .expect("identity parameter index out of range");
            param_marker_bounds.push(quote! {
                #declared_ty: #krate::extract::FromRequestPartsVia<#state_ident, #marker>
            });
            param_markers.push(marker);
        };
        for rm in &def.route_methods {
            if let Some(ref p) = rm.identity_param {
                push_identity(&rm.fn_item, p.index);
            }
        }
        for sm in &def.sse_methods {
            if let Some(ref p) = sm.identity_param {
                push_identity(&sm.fn_item, p.index);
            }
        }
        for wm in &def.ws_methods {
            if let Some(ref p) = wm.identity_param {
                push_identity(&wm.fn_item, p.index);
            }
        }
    }

    // Managed resource bounds, deduplicated by type tokens.
    let mut managed_seen = std::collections::HashSet::new();
    let mut managed_bounds: Vec<TokenStream> = Vec::new();
    for rm in &def.route_methods {
        for mp in &rm.managed_params {
            let ty = crate::type_utils::staticize_lifetimes(&mp.ty);
            if managed_seen.insert(quote!(#ty).to_string()) {
                managed_bounds.push(quote! { #ty: #krate::ManagedResource<#state_ident> });
            }
        }
    }

    quote! {
        // Shared controller-level interceptor set (module scope): one struct +
        // constructor, one instance per surface.
        #ctrl_deco_items

        // Transverse decorator sets + container/fill impl and the
        // ScheduledSource/EventSubscriber/PostConstruct impls (module scope:
        // the container type is downcast in the intercepted method bodies).
        #transverse_items

        #fallback_prefix_assert

        // State-independent carrier of the full dep list (core ++ decorator
        // deps) — lets `register_module` check decorator deps in the NoState
        // phase, where `Controller<S, W>::Deps` is not yet nameable.
        #[doc(hidden)]
        impl #krate::EndpointDeps for #name {
            type Deps = #deps_fold;
        }

        impl<#state_ident, #md, #(#param_markers),*>
            #krate::Controller<#state_ident, (#md, #(#param_markers,)*)> for #name
        where
            #state_ident: #state_bounds,
            #md: Send + Sync + 'static,
            #(#param_markers: Send + Sync + 'static,)*
            #data_name<#md>: #krate::http::extract::FromRequestParts<#state_ident>,
            #(#param_marker_bounds,)*
            #(#managed_bounds,)*
        {
            type Deps = <#name as #krate::EndpointDeps>::Deps;

            fn construct(_state: &#state_ident, __ctx: &#krate::beans::BeanContext) -> Self {
                <#name as #krate::ContextConstruct>::from_context(__ctx)
            }

            fn routes(
                __state: &#state_ident,
                __core: ::std::sync::Arc<Self>,
                __ctx: &#krate::beans::BeanContext,
            ) -> #krate::http::Router<#state_ident> {
                (#application_router_body)(__core, __ctx)
            }

            fn register_meta(__registry: &mut #krate::meta::MetaRegistry) {
                #(#register_meta_stmts)*
            }

            #transverse_fns

            fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                #meta_mod::validate_config(__config)
            }
        }
    }
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
        // Proxy-shaped routes have no documentable OpenAPI operation:
        // `#[fallback]` matches whatever is left over, `#[any]` has no single
        // method, and `{*wildcard}` paths are not valid OpenAPI path templates.
        .filter(|rm| {
            !rm.is_fallback
                && rm.method != crate::route::HttpMethod::Any
                && !is_wildcard_path(&rm.path)
        })
        .map(|rm| {
            let route_path_str = &rm.path;
            let method = rm.method.as_routing_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            let roles: Vec<_> = rm.decorators.roles.iter().chain(rm.decorators.all_roles.iter()).map(|r| quote! { #r.to_string() }).collect();
            let tag = &tag_name;

            let path_params = extract_path_params(rm, &krate);
            let handler_param_types = extract_handler_param_types(rm);
            let (body_type_token, body_schema_token, body_content_type_token) =
                extract_body_info(rm);
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

            let has_roles = !rm.decorators.roles.is_empty() || !rm.decorators.all_roles.is_empty();
            let has_identity_param = rm.identity_param.is_some();
            let has_guards = !rm.decorators.guard_fns.is_empty();
            let has_auth = has_auth_expr(
                rm.decorators.anonymous,
                has_roles,
                has_identity_param,
                has_guards,
                meta_mod,
            );

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
                    request_body_content_type: #body_content_type_token,
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
                    has_auth: #has_auth,
                }
            }
        })
        .collect()
}

/// The `RouteInfo.has_auth` expression for a route.
///
/// Normal routes: roles, an identity param, guard fns, or the struct-level
/// identity all mark the operation as secured. `#[anonymous]` routes bypass
/// the struct identity, cannot carry roles or a *required* identity param
/// (rejected at parse time), and an *optional* identity param never rejects —
/// so only explicit guards (which may still reject, e.g. an API-key check)
/// keep the flag on.
fn has_auth_expr(
    anonymous: bool,
    has_roles: bool,
    has_identity_param: bool,
    has_guards: bool,
    meta_mod: &syn::Ident,
) -> TokenStream {
    if anonymous {
        quote! { #has_guards }
    } else {
        quote! { #has_roles || #has_identity_param || #has_guards || #meta_mod::HAS_STRUCT_IDENTITY }
    }
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
                if type_last_segment_is(ty, "Path") {
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
/// Shared with the `FromMultipart` derive so path params and multipart text
/// fields classify primitives identically.
pub(crate) fn type_to_openapi_str(ty: &syn::Type) -> &'static str {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return match segment.ident.to_string().as_str() {
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
                | "i128" | "isize" => "integer",
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
        crate::route::HttpMethod::Any => 200,
    }
}

/// Whether a route path contains an axum `{*wildcard}` segment. Such paths
/// are not valid OpenAPI path templates, so the route is excluded from the spec.
fn is_wildcard_path(path: &str) -> bool {
    path.contains("{*")
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

/// Emit an autoref-specialization schema probe: when `ty` satisfies `bound`,
/// the inherent method wins and returns `Some(#some_body)` (with `T` bound to
/// `ty`); otherwise the trait fallback returns `None`. Lets optional schema
/// discovery work without requiring the bound on every type.
fn autoref_schema_probe(ty: &syn::Type, bound: TokenStream, some_body: TokenStream) -> TokenStream {
    let krate = r2e_core_path();
    quote! {
        {
            struct __SchemaProbe<T>(::core::marker::PhantomData<T>);
            trait __NoSchema {
                fn __schema(&self) -> Option<#krate::serde_json::Value> { None }
            }
            impl<T> __NoSchema for &__SchemaProbe<T> {}
            impl<T: #bound> __SchemaProbe<T> {
                fn __schema(&self) -> Option<#krate::serde_json::Value> {
                    Some(#some_body)
                }
            }
            let __p = __SchemaProbe::<#ty>(::core::marker::PhantomData);
            use __NoSchema as _;
            (&__p).__schema()
        }
    }
}

/// Generate a schema token for a response type, using autoref specialization
/// so that types not implementing `JsonSchema` gracefully return `None`.
fn response_schema_token(ty: &syn::Type) -> TokenStream {
    if let Some(schemars) = crate::crate_path::r2e_schemars_path() {
        let krate = r2e_core_path();
        autoref_schema_probe(
            ty,
            quote! { #schemars::JsonSchema },
            quote! { #krate::serde_json::to_value(#schemars::schema_for!(T)).unwrap() },
        )
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

/// A handler parameter recognized as the request body extractor.
enum BodyExtractor {
    /// `Json<T>` — `application/json` with a schemars-generated schema.
    Json { name: String, ty: syn::Type },
    /// `TypedMultipart<T>` — `multipart/form-data` with a `MultipartSchema`-probed schema.
    TypedMultipart { name: String, ty: syn::Type },
    /// Raw `Multipart` — `multipart/form-data`, free-form (no named schema).
    RawMultipart,
}

/// Media type emitted for multipart body extractors.
const MULTIPART_CONTENT_TYPE: &str = "multipart/form-data";

/// Extract request body information.
/// Returns (type_name_token, schema_token, content_type_token).
fn extract_body_info(rm: &crate::types::RouteMethod) -> (TokenStream, TokenStream, TokenStream) {
    let body_info: Option<BodyExtractor> = rm.fn_item.sig.inputs.iter().find_map(|arg| {
        if let syn::FnArg::Typed(pt) = arg {
            extract_body_type_info(&pt.ty)
        } else {
            None
        }
    });

    let multipart_ct = MULTIPART_CONTENT_TYPE;
    match &body_info {
        Some(BodyExtractor::Json { name, ty }) => {
            let schema_token = if let Some(schemars) = crate::crate_path::r2e_schemars_path() {
                let krate = r2e_core_path();
                quote! {
                    Some({
                        let __schema = #schemars::schema_for!(#ty);
                        #krate::serde_json::to_value(__schema).unwrap()
                    })
                }
            } else {
                quote! { None }
            };
            (quote! { Some(#name.to_string()) }, schema_token, quote! { None })
        }
        Some(BodyExtractor::TypedMultipart { name, ty }) => (
            quote! { Some(#name.to_string()) },
            multipart_schema_token(ty),
            quote! { Some(#multipart_ct.to_string()) },
        ),
        Some(BodyExtractor::RawMultipart) => (
            quote! { None },
            quote! { None },
            quote! { Some(#multipart_ct.to_string()) },
        ),
        None => (quote! { None }, quote! { None }, quote! { None }),
    }
}

/// Generate a schema token for a `TypedMultipart<T>` body via autoref
/// specialization: the derived `MultipartSchema` impl yields `Some(schema)`;
/// a manual `FromMultipart` impl without it degrades to `None`.
///
/// The `MultipartSchema` trait lives in `r2e_core::meta` (always compiled,
/// not the feature-gated `multipart` module) so this probe also compiles in
/// apps that use a `TypedMultipart`-shaped extractor without the feature.
fn multipart_schema_token(ty: &syn::Type) -> TokenStream {
    let krate = r2e_core_path();
    autoref_schema_probe(
        ty,
        quote! { #krate::meta::MultipartSchema },
        quote! { <T as #krate::meta::MultipartSchema>::multipart_schema() },
    )
}

/// Classify a handler parameter type as a body extractor.
fn extract_body_type_info(ty: &syn::Type) -> Option<BodyExtractor> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            match ident.as_str() {
                "Json" | "TypedMultipart" => {
                    if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                            if let syn::Type::Path(inner_path) = inner_ty {
                                if let Some(inner_seg) = inner_path.path.segments.last() {
                                    let name = inner_seg.ident.to_string();
                                    let ty = inner_ty.clone();
                                    return Some(if ident == "Json" {
                                        BodyExtractor::Json { name, ty }
                                    } else {
                                        BodyExtractor::TypedMultipart { name, ty }
                                    });
                                }
                            }
                        }
                    }
                }
                "Multipart" => return Some(BodyExtractor::RawMultipart),
                _ => {}
            }
        }
    }
    None
}

/// Generate the off-request (transverse) wiring for a controller core.
///
/// Emits, at module scope, the decorator sets + container + `BeanDecoFill` for
/// intercepted `#[scheduled]`/`#[consumer]` methods, plus the `ScheduledSource`
/// / `EventSubscriber` impls (for `Arc<Core>` — cores live behind one `Arc` and
/// may not be `Clone`, so the task/consumer closures clone the `Arc`) and the
/// `PostConstruct` impl (for the core). Also returns the `Controller` method
/// overrides (`register_consumers`, `scheduled_tasks_boxed`, `fill_decos`,
/// `post_construct`) that delegate to those impls — each emitted only when the
/// controller actually has the relevant methods, so the trait defaults apply
/// otherwise.
///
/// The container + slot are the same `sched_container_ident` /
/// `sched_field_ident` the dispatch wrappers in `wrapping.rs` read, so direct
/// in-code calls run the chain too.
fn generate_transverse(def: &RoutesImplDef, name: &syn::Ident) -> (TokenStream, TokenStream) {
    let krate = r2e_core_path();
    let state_ident = super::handlers::state_generic();
    let owner_name = name.to_string();

    let mut module_items: Vec<TokenStream> = Vec::new();
    // One container field per METHOD-level-intercepted transverse method
    // (scheduled OR consumer) — drives the container struct + fill impl.
    let mut deco_fields: Vec<DecoFieldDef> = Vec::new();

    // Shared controller-level (impl-level) interceptor set, applied to the
    // transverse surface when the controller has such interceptors and at least
    // one scheduled/consumer method. Built ONCE at fill (stored in the
    // container's `__ctrl` field) so every transverse method shares one instance.
    let ctrl_set = super::decorators::ctrl_deco_set(def);
    let ctrl_for_transverse = ctrl_set.is_some()
        && (!def.scheduled_methods.is_empty() || !def.consumer_methods.is_empty());

    // ── Scheduled decorator sets (method-level interceptors only) ──
    let mut sched_sets: Vec<Option<super::decorators::DecoSet>> = Vec::new();
    for sm in &def.scheduled_methods {
        let intercept_exprs: Vec<&syn::Expr> = sm.intercept_fns.iter().collect();
        let (items, set) = super::decorators::generate_named_deco_items(
            name,
            "Sched",
            &sm.fn_item.sig.ident,
            &[],
            &intercept_exprs,
            quote! {},
        );
        module_items.push(items);
        if let Some(ref s) = set {
            deco_fields.push(DecoFieldDef {
                field: super::decorators::sched_field_ident(&sm.fn_item.sig.ident),
                set_ty: s.ty().clone(),
                ctor: s.ctor_ident.clone(),
            });
        }
        sched_sets.push(set);
    }

    // ── Consumer decorator sets (method-level interceptors only) ──
    for cm in &def.consumer_methods {
        let intercept_exprs: Vec<&syn::Expr> = cm.intercept_fns.iter().collect();
        let (items, set) = super::decorators::generate_named_deco_items(
            name,
            "Cons",
            &cm.fn_item.sig.ident,
            &[],
            &intercept_exprs,
            quote! {},
        );
        module_items.push(items);
        if let Some(ref s) = set {
            deco_fields.push(DecoFieldDef {
                field: super::decorators::sched_field_ident(&cm.fn_item.sig.ident),
                set_ty: s.ty().clone(),
                ctor: s.ctor_ident.clone(),
            });
        }
    }

    // The container/fill is needed for method-level sets OR the shared
    // controller-level set.
    let has_decos = !deco_fields.is_empty() || ctrl_for_transverse;

    // ── Container + BeanDecoFill for the core (slot = the `DecoSlot` field) ──
    if has_decos {
        let container = super::decorators::sched_container_ident(name);
        let slot_access = quote! { self.__r2e_decos };
        let ctrl_field = ctrl_for_transverse.then(|| {
            let set = ctrl_set.as_ref().expect("ctrl_for_transverse implies ctrl_set");
            transverse::CtrlContainerField {
                set_ty: set.struct_ident.clone(),
                ctor: set.ctor_ident.clone(),
            }
        });
        module_items.push(transverse::deco_container_and_fill(
            &container,
            &quote! { #name },
            &slot_access,
            &deco_fields,
            ctrl_field.as_ref(),
        ));
    }

    // ── PostConstruct impl for the core ──
    if !def.post_construct_methods.is_empty() {
        module_items.push(transverse::post_construct_impl(
            &quote! { #name },
            &def.post_construct_methods,
        ));
    }

    // ── Controller method overrides ──
    //
    // A controller core is not a legal `ScheduledSource`/`EventSubscriber` impl
    // target (`Arc<Core>` breaks the orphan rule, and the bare core is not
    // `Clone`), so the task-def / subscribe-block bodies are embedded directly
    // in the overrides, cloning the passed `Arc<Self>` core.
    let mut controller_fns: Vec<TokenStream> = Vec::new();

    if !def.consumer_methods.is_empty() {
        let consumers: Vec<ConsumerMethodDef> = def
            .consumer_methods
            .iter()
            .map(|cm| ConsumerMethodDef {
                bus_field: format_ident!("{}", cm.bus_field),
                event_type: cm.event_type.clone(),
                fn_name: cm.fn_item.sig.ident.clone(),
                kind: cm.kind.clone(),
                topic: cm.topic.clone(),
                deserializer: cm.deserializer.clone(),
                filter: cm.filter.clone(),
                retry: cm.retry,
                dlq: cm.dlq.clone(),
            })
            .collect();
        // Custom `deserializer` assoc fns live on the concrete core.
        let blocks =
            transverse::event_subscribe_blocks(&quote! { __core }, &quote! { #name }, &consumers);
        controller_fns.push(quote! {
            fn register_consumers(
                _state: #state_ident,
                __core: ::std::sync::Arc<Self>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    #(#blocks)*
                })
            }
        });
    }

    if !def.scheduled_methods.is_empty() {
        let methods: Vec<ScheduledSourceMethod> = def
            .scheduled_methods
            .iter()
            .zip(sched_sets.iter())
            .map(|(sm, set)| ScheduledSourceMethod {
                fn_name: sm.fn_item.sig.ident.clone(),
                config: sm.config.clone(),
                // Intercepted methods self-intercept in their dispatch wrapper
                // (sync sources promoted to `async fn`), so the emitted call is
                // awaited when the source is async OR it runs method-level
                // interceptors OR it runs the shared controller-level ones.
                emitted_async: sm.fn_item.sig.asyncness.is_some()
                    || set.is_some()
                    || ctrl_for_transverse,
            })
            .collect();
        let task_defs = transverse::scheduled_task_defs(&quote! { __core }, &owner_name, &methods);
        // On the manual (test) path `scheduled_tasks_boxed` is called without a
        // prior `register_controller`, so fill the slot here too before building
        // tasks. Registration already filled it via `fill_decos`; the slot's
        // `OnceLock` makes the repeat a no-op. Emitted only when a slot exists.
        let slot_fill = if has_decos {
            quote! {
                #krate::BeanDecoFill::__r2e_fill_decos(&*__core, __ctx);
            }
        } else {
            quote! {}
        };
        controller_fns.push(quote! {
            fn scheduled_tasks_boxed(
                _state: &#state_ident,
                __core: ::std::sync::Arc<Self>,
                __ctx: &#krate::beans::BeanContext,
            ) -> Vec<Box<dyn std::any::Any + Send>> {
                #slot_fill
                vec![#(#task_defs),*]
            }
        });
    }

    if has_decos {
        controller_fns.push(quote! {
            fn fill_decos(__core: &::std::sync::Arc<Self>, __ctx: &#krate::beans::BeanContext) {
                #krate::BeanDecoFill::__r2e_fill_decos(&**__core, __ctx);
            }
        });
    }

    if !def.post_construct_methods.is_empty() {
        controller_fns.push(quote! {
            fn post_construct(
                __core: ::std::sync::Arc<Self>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> {
                Box::pin(async move {
                    #krate::beans::PostConstruct::post_construct(&*__core).await
                })
            }
        });
    }

    // `#[pre_destroy]` disposal hooks. Controller cores are not `Clone`, so they
    // cannot impl the `PreDestroy` trait (its supertrait); the disposal calls are
    // inlined here, run from the core `Arc` at shutdown. An `Err` is logged and
    // swallowed (disposal never aborts shutdown).
    if !def.pre_destroy_methods.is_empty() {
        let calls =
            transverse::pre_destroy_calls(&quote! { __self }, &owner_name, &def.pre_destroy_methods);
        controller_fns.push(quote! {
            const HAS_PRE_DESTROY: bool = true;

            fn pre_destroy(
                __core: ::std::sync::Arc<Self>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    let __self: &Self = &*__core;
                    #(#calls)*
                })
            }
        });
    }

    (quote! { #(#module_items)* }, quote! { #(#controller_fns)* })
}

fn generate_sse_route_metadata(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    def.sse_methods
        .iter()
        .map(|sm| {
            emit_streaming_route_info(
                name,
                meta_mod,
                &sm.path,
                &sm.fn_item.sig.ident,
                &sm.decorators.roles,
                &sm.decorators.all_roles,
                !sm.decorators.guard_fns.is_empty(),
                sm.identity_param.is_some(),
                sm.decorators.anonymous,
                "SSE stream",
            )
        })
        .collect()
}

fn generate_ws_route_metadata(
    def: &RoutesImplDef,
    name: &syn::Ident,
    meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    def.ws_methods
        .iter()
        .map(|wm| {
            emit_streaming_route_info(
                name,
                meta_mod,
                &wm.path,
                &wm.fn_item.sig.ident,
                &wm.decorators.roles,
                &wm.decorators.all_roles,
                !wm.decorators.guard_fns.is_empty(),
                wm.identity_param.is_some(),
                wm.decorators.anonymous,
                "WebSocket endpoint",
            )
        })
        .collect()
}

/// Emit a `RouteInfo` literal for SSE / WS routes.
///
/// Both emit a `GET` with empty body/params/response and a 200 status; they
/// differ only in summary text. Keeping this in one place makes adding a new
/// streaming route kind (or a new `RouteInfo` field) a single-edit affair.
#[allow(clippy::too_many_arguments)]
fn emit_streaming_route_info(
    controller_name: &syn::Ident,
    meta_mod: &syn::Ident,
    path: &str,
    fn_ident: &syn::Ident,
    roles: &[String],
    all_roles: &[String],
    has_guards: bool,
    has_identity_param: bool,
    anonymous: bool,
    summary: &str,
) -> TokenStream {
    let krate = r2e_core_path();
    let tag = controller_name.to_string();
    let op_id = format!("{}_{}", controller_name, fn_ident);
    let roles_tokens: Vec<_> = roles
        .iter()
        .chain(all_roles.iter())
        .map(|r| quote! { #r.to_string() })
        .collect();
    let has_roles = !roles.is_empty() || !all_roles.is_empty();
    let has_auth = has_auth_expr(anonymous, has_roles, has_identity_param, has_guards, meta_mod);

    quote! {
        #krate::meta::RouteInfo {
            path: match #meta_mod::PATH_PREFIX {
                Some(__prefix) => format!("{}{}", __prefix, #path),
                None => #path.to_string(),
            },
            method: "GET".to_string(),
            operation_id: #op_id.to_string(),
            summary: Some(#summary.to_string()),
            description: None,
            request_body_type: None,
            request_body_schema: None,
            request_body_content_type: None,
            request_body_required: true,
            response_type: None,
            response_schema: None,
            response_status: 200,
            params: vec![],
            roles: vec![#(#roles_tokens),*],
            tag: Some(#tag.to_string()),
            deprecated: false,
            has_auth: #has_auth,
        }
    }
}

// ── Application-scoped route registrations ─────────────────────────────
//
// These produce the `.route(path, METHOD(closure))` fragments registered
// inside the state-aware application-controller closure. Each fragment
// captures the controller `Arc` once and forwards to the common handler
// wrapper emitted by `handlers.rs`.

fn generate_route_registrations(def: &RoutesImplDef) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    def.route_methods
        .iter()
        .filter(|rm| rm.decorators.pre_auth_guard_fns.is_empty())
        .map(|rm| {
            let path = &rm.path;
            let method_fn = format_ident!("{}", rm.method.as_routing_fn());
            let closure = super::handlers::generate_route_closure(def, rm);
            let middleware_layers: Vec<_> = rm
                .decorators
                .middleware_fns
                .iter()
                .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
                .collect();
            let direct_layers: Vec<_> = rm
                .decorators
                .layer_exprs
                .iter()
                .map(|expr| quote! { .layer(#expr) })
                .collect();
            if rm.is_fallback {
                // #[fallback]: handles everything no other route matched.
                // #[middleware]/#[layer]/#[pre_guard] are rejected at parse
                // time, so the closure is registered bare.
                quote! {
                    .fallback(#closure)
                }
            } else {
                quote! {
                    .route(
                        #path,
                        #krate::http::routing::#method_fn(#closure)
                            #(#middleware_layers)*
                            #(#direct_layers)*
                    )
                }
            }
        })
        .collect()
}

fn generate_sse_route_registrations(def: &RoutesImplDef) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    def.sse_methods
        .iter()
        .filter(|sm| sm.decorators.pre_auth_guard_fns.is_empty())
        .map(|sm| {
            let path = &sm.path;
            let closure = super::handlers::generate_sse_closure(def, sm);
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
                    #krate::http::routing::get(#closure)
                        #(#middleware_layers)*
                        #(#direct_layers)*
                )
            }
        })
        .collect()
}

fn generate_ws_route_registrations(def: &RoutesImplDef) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    def.ws_methods
        .iter()
        .filter(|wm| wm.decorators.pre_auth_guard_fns.is_empty())
        .map(|wm| {
            let path = &wm.path;
            let closure = super::handlers::generate_ws_closure(def, wm);
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
                    #krate::http::routing::get(#closure)
                        #(#middleware_layers)*
                        #(#direct_layers)*
                )
            }
        })
        .collect()
}

/// Generate `__inner = __inner.route(...);` statements wrapping
/// pre-auth-guarded routes with the captured-core closure + pre-auth middleware.
/// Paths are bare here because the
/// surrounding `match PATH_PREFIX` re-nests the router afterwards.
///
/// Pre-auth guards are prebuilt (once, from the bean context) into the
/// method's `__R2ePreDeco_*` set; the middleware closure captures one `Arc`
/// of it — no state access, no per-request construction.
fn generate_pre_auth_registrations(
    def: &RoutesImplDef,
    name: &syn::Ident,
    _meta_mod: &syn::Ident,
) -> Vec<TokenStream> {
    let mut registrations: Vec<TokenStream> = Vec::new();

    for rm in &def.route_methods {
        if rm.decorators.pre_auth_guard_fns.is_empty() {
            continue;
        }
        let method_fn = format_ident!("{}", rm.method.as_routing_fn());
        registrations.push(pre_auth_registration(
            def,
            name,
            &rm.fn_item.sig.ident,
            &rm.path,
            &rm.decorators,
            quote! { #method_fn },
            super::handlers::generate_route_closure(def, rm),
        ));
    }
    // SSE/WS endpoints run their pre-auth guards through the same middleware.
    for sm in &def.sse_methods {
        if sm.decorators.pre_auth_guard_fns.is_empty() {
            continue;
        }
        registrations.push(pre_auth_registration(
            def,
            name,
            &sm.fn_item.sig.ident,
            &sm.path,
            &sm.decorators,
            quote! { get },
            super::handlers::generate_sse_closure(def, sm),
        ));
    }
    for wm in &def.ws_methods {
        if wm.decorators.pre_auth_guard_fns.is_empty() {
            continue;
        }
        registrations.push(pre_auth_registration(
            def,
            name,
            &wm.fn_item.sig.ident,
            &wm.path,
            &wm.decorators,
            quote! { get },
            super::handlers::generate_ws_closure(def, wm),
        ));
    }
    registrations
}

fn pre_auth_registration(
    def: &RoutesImplDef,
    name: &syn::Ident,
    fn_ident: &syn::Ident,
    path: &str,
    decorators: &crate::types::MethodDecorators,
    method_fn: TokenStream,
    closure: TokenStream,
) -> TokenStream {
    let krate = r2e_core_path();

    // Mirror the post-auth degrade: when a pre-guard spec type is not
    // inferable, `generate_predeco_items` emitted the compile_error and no
    // ctor — register the route without the pre-auth layer so the only
    // error the user sees is the spec-type one.
    if !super::decorators::all_specs_inferable(decorators.pre_auth_guard_fns.iter()) {
        let middleware_layers: Vec<_> = decorators
            .middleware_fns
            .iter()
            .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
            .collect();
        let direct_layers: Vec<_> = decorators
            .layer_exprs
            .iter()
            .map(|expr| quote! { .layer(#expr) })
            .collect();
        return quote! {
            __inner = __inner.route(
                #path,
                #krate::http::routing::#method_fn(#closure)
                    #(#middleware_layers)*
                    #(#direct_layers)*
            );
        };
    }

    let controller_name_str = name.to_string();
    let fn_name_str = fn_ident.to_string();
    let controller_name = &def.controller_name;
    let predeco_ctor = format_ident!("__r2e_predeco_{}_{}", controller_name, fn_ident);

    let pre_auth_checks: Vec<_> = (0..decorators.pre_auth_guard_fns.len())
        .map(|i| {
            let field = format_ident!("__p{}", i);
            quote! {
                if let Err(__resp) = #krate::PreAuthGuard::check(
                    &__pre_deco.#field,
                    &__pre_ctx,
                ).await {
                    return __resp;
                }
            }
        })
        .collect();

    let middleware_layers: Vec<_> = decorators
        .middleware_fns
        .iter()
        .map(|mw_fn| quote! { .layer(#krate::http::middleware::from_fn(#mw_fn)) })
        .collect();
    let direct_layers: Vec<_> = decorators
        .layer_exprs
        .iter()
        .map(|expr| quote! { .layer(#expr) })
        .collect();

    quote! {
        {
            let __pre_deco_capture = ::std::sync::Arc::new(#predeco_ctor(__ctx));
            let __pre_auth_mw = move |__req: #krate::http::extract::Request,
                                      __next: #krate::http::middleware::Next| {
                let __pre_deco = __pre_deco_capture.clone();
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
            __inner = __inner.route(
                #path,
                #krate::http::routing::#method_fn(#closure)
                    #(#middleware_layers)*
                    #(#direct_layers)*
                    .layer(#krate::http::middleware::from_fn(__pre_auth_mw))
            );
        }
    }
}

