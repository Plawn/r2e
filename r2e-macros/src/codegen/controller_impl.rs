//! Controller trait implementation generation.

use proc_macro2::TokenStream;

use quote::{format_ident, quote};

use crate::codegen::transverse::{self, ConsumerMethodDef, DecoFieldDef, ScheduledSourceMethod};
use crate::parsing::routes_parsing::RoutesImplDef;
use crate::util::crate_path::r2e_core_path;
use crate::util::type_utils::{type_last_segment_is, unwrap_option_type, unwrap_result_type};

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
    // Controller deps = core `ContextConstruct::Deps` ++ every decorator
    // site's `<Spec as DecoratorSpec>::Deps`. Emitted once, on the
    // `EndpointDeps` carrier — checked by `AllSatisfied` at
    // `register_controller()` and by `ModuleDepsSatisfied` at
    // `register_module()`.
    let deps_fold = super::decorators::controller_deps_fold(def);
    // Same site set, config side: a decorator bean's `#[config]` keys are
    // declared on its `DecoratorSpec` but only the host knows the sites, so the
    // controller's aggregated `validate_config` reports them at
    // `register_controller()` — before any `build_decorator` runs.
    let decorator_config_stmts = super::decorators::decorator_config_key_stmts(
        super::decorators::controller_site_exprs(def),
    );
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

    // Controller-level (impl-level) decorator products (`#[intercept]`,
    // `#[guard]`/`#[roles]`/`#[all_roles]`, `#[pre_guard]`), built ONCE per
    // controller and shared (via `Arc` clones) across every HTTP surface — so
    // a stateful impl-level decorator keeps a single instance, not one per
    // route. Emitted when some surface actually captures it: any route (they
    // take the set whenever it exists — interceptors and/or guards), or an
    // SSE/WS endpoint when the set carries guards, or any endpoint when it
    // carries pre-guards (the entry function runs them first).
    let ctrl_deco_items = super::decorators::generate_ctrl_deco_items(def);
    let ctrl_router_setup = super::decorators::ctrl_deco_set(def)
        .filter(|set| {
            let has_sse_ws = !def.sse_methods.is_empty() || !def.ws_methods.is_empty();
            !def.route_methods.is_empty()
                || (has_sse_ws
                    && (!set.guard_fields.is_empty() || !set.pre_guard_fields.is_empty()))
        })
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
                #declared_ty: #krate::web::extract::FromRequestPartsVia<#state_ident, #marker>
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
            let ty = crate::util::type_utils::staticize_lifetimes(&mp.ty);
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
            #data_name<#md>: #krate::web::extract::RequestData<#state_ident>,
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

            fn register_meta(__registry: &mut #krate::di::meta::MetaRegistry) {
                #(#register_meta_stmts)*
            }

            #transverse_fns

            fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> Vec<#krate::config::MissingKeyError> {
                #[allow(unused_mut)]
                let mut __errors = #meta_mod::validate_config(__config);
                #(#decorator_config_stmts)*
                __errors
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

    def.route_methods
        .iter()
        // Proxy-shaped routes have no documentable OpenAPI operation:
        // `#[fallback]` matches whatever is left over, `#[any]` has no single
        // method, and `{*wildcard}` paths are not valid OpenAPI path templates.
        .filter(|rm| {
            !rm.is_fallback
                && rm.method != crate::model::route::HttpMethod::Any
                && !is_wildcard_path(&rm.path)
        })
        .map(|rm| {
            let route_path_str = &rm.path;
            let method = rm.method.as_routing_fn().to_uppercase();
            let op_id = format!("{}_{}", name, rm.fn_item.sig.ident);
            // Controller-level #[roles]/#[all_roles]/#[guard] apply to every
            // non-#[anonymous] route (anonymous opts out of the controller's
            // post-auth checks), so their metadata folds in on the same rule.
            let ctrl = &def.controller_decorators;
            let ctrl_applies = !rm.decorators.anonymous;
            let mut role_strs: Vec<&String> = rm
                .decorators
                .roles
                .iter()
                .chain(rm.decorators.all_roles.iter())
                .collect();
            if ctrl_applies {
                role_strs.extend(ctrl.roles.iter().chain(ctrl.all_roles.iter()));
            }
            let roles: Vec<_> = role_strs.iter().map(|r| quote! { #r.to_string() }).collect();

            let params_expr = params_expr(&rm.fn_item.sig, None, &krate);
            let request = super::handlers::RequestParams::route(rm);
            let extracted = request.owned();
            let body = extract_body_info(&extracted, request.last_consumes_body, &rm.method);
            let body_probe = body.probe_expr();
            let body_required = body.required;
            let body_unmapped_token = body.unmapped_token();
            let response = response_info(rm);
            let response_probe = &response.probe;
            let response_unmapped_token = match &response.unmapped {
                Some(name) => quote! { Some(#name.to_string()) },
                None => quote! { None },
            };

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

            let has_roles = !role_strs.is_empty();
            let static_kinds = static_rejection_kinds(
                def,
                &rm.decorators,
                rm.identity_param.as_ref(),
                has_roles,
                &extracted,
                request.last_consumes_body,
                &rm.method,
            );
            let kinds_expr = rejection_kinds_expr(
                &static_kinds,
                rm.decorators.anonymous,
                &extracted,
                meta_mod,
            );
            let error_schema =
                super::handlers::Projection::for_signature(&rm.fn_item.sig).schema_expr(&krate);

            quote! {
                {
                    let __params: Vec<#krate::di::meta::ParamInfo> = #params_expr;
                    let __body: Option<#krate::di::meta::__BodyProbeResult> = #body_probe;
                    let __kinds: Vec<#krate::RejectionKind> = #kinds_expr;
                    // `None`: the return type has no `ResponseBodySchema` impl
                    // (or is opaque) → unmapped. `Some(vec![])`: no body.
                    let __response: Option<Vec<#krate::di::meta::ResponseContent>> = #response_probe;
                    #krate::di::meta::RouteInfo {
                        path: match #meta_mod::PATH_PREFIX {
                            Some(__prefix) => format!("{}{}", __prefix, #route_path_str),
                            None => #route_path_str.to_string(),
                        },
                        method: #method.to_string(),
                        operation_id: #op_id.to_string(),
                        summary: #summary_token,
                        description: #description_token,
                        request_body_unmapped: if __body.is_none() {
                            #body_unmapped_token
                        } else {
                            None
                        },
                        request_body: __body.map(|__b| #krate::di::meta::RequestBody {
                            content_type: __b.content_type.to_string(),
                            schema: __b.schema,
                            required: #body_required,
                        }),
                        response_status: #status_code,
                        response_unmapped: if __response.is_none() {
                            #response_unmapped_token
                        } else {
                            None
                        },
                        response_contents: __response.unwrap_or_default(),
                        params: __params,
                        roles: vec![#(#roles),*],
                        tag: Some(#meta_mod::OPENAPI_TAG.to_string()),
                        deprecated: #deprecated,
                        rejection_kinds: __kinds,
                        error_schema: #error_schema,
                    }
                }
            }
        })
        .collect()
}

/// Whether a guard expression names a rate-limit guard (`RateLimit::per_user(..)`,
/// `PreRateLimit`, `ConfiguredRateLimit`, …): its only failure is `RateLimited`.
fn is_rate_limit_guard(expr: &syn::Expr) -> bool {
    super::decorators::spec_type_of(expr)
        .ok()
        .and_then(|(path, _)| path.segments.last().map(|s| s.ident.to_string()))
        .is_some_and(|name| name.contains("RateLimit"))
}

/// Rejection kinds known at macro time, as `RejectionKind` variant names.
///
/// Inference table (see `plans/error-projection.md` §6): the body extractor's
/// failures; `Path<T>` → `InvalidPath`, `Query<T>` → `InvalidQuery`, `Form<T>`
/// → `InvalidForm` on a GET (query string) or `InvalidBody` (422) + media-type,
/// size and read failures on a body-carrying method (both on `#[any]`), raw
/// `Bytes`/`String` bodies → read failures; an identity parameter →
/// `Unauthenticated`, `Option<..>` included (an absent credential is `None`,
/// but a present invalid one is still a 401);
/// roles → `Forbidden`; each guard → `Forbidden`, or `RateLimited` when it is
/// a rate-limit guard; and `Internal` always (handler, managed resources,
/// anything the framework cannot type). Controller-level guards fold in for
/// non-`#[anonymous]` routes only (the same rule as their execution);
/// pre-auth guards run on every route.
///
/// Runtime-only facts (struct identity, `#[derive(Params)]` locations, garde
/// validation, a custom body extractor) are added by [`rejection_kinds_expr`].
fn static_rejection_kinds(
    def: &RoutesImplDef,
    decorators: &crate::model::types::MethodDecorators,
    identity_param: Option<&crate::model::types::IdentityParam>,
    has_roles: bool,
    extracted: &[syn::PatType],
    last_consumes_body: bool,
    method: &crate::model::route::HttpMethod,
) -> Vec<&'static str> {
    use crate::model::route::HttpMethod;
    let mut kinds: Vec<&'static str> = Vec::new();

    // Body extractors declare their kinds through `RequestBodySchema`, read
    // at runtime by `rejection_kinds_expr` (`__body`). Only the non-body
    // extractors are known here by name.
    let n = extracted.len();
    for (pos, pt) in extracted.iter().enumerate() {
        let last = last_consumes_body && pos + 1 == n;
        if type_last_segment_is(&pt.ty, "Path") {
            kinds.push("InvalidPath");
        } else if type_last_segment_is(&pt.ty, "Query") {
            kinds.push("InvalidQuery");
        } else if last
            && type_last_segment_is(&pt.ty, "Form")
            && matches!(method, HttpMethod::Get | HttpMethod::Any)
        {
            // axum's `Form` reads the query string on GET/HEAD (400); on other
            // methods it is the body and `Form<T>: RequestBodySchema` applies.
            kinds.push("InvalidForm");
        }
    }

    if identity_param.is_some() {
        kinds.push("Unauthenticated");
    }
    if has_roles {
        kinds.push("Forbidden");
    }

    let ctrl = &def.controller_decorators;
    let post_auth = decorators.guard_fns.iter().chain(
        (!decorators.anonymous)
            .then_some(ctrl.guard_fns.iter())
            .into_iter()
            .flatten(),
    );
    let pre_auth = decorators
        .pre_auth_guard_fns
        .iter()
        .chain(ctrl.pre_auth_guard_fns.iter());
    for guard in post_auth.chain(pre_auth) {
        kinds.push(if is_rate_limit_guard(guard) {
            "RateLimited"
        } else {
            "Forbidden"
        });
    }

    kinds.push("Internal");
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// The type garde validation runs on for a handler parameter: the inner `T`
/// of a `Json<T>` / `Query<T>` / `Path<T>` / `Form<T>` wrapper (the entry
/// function validates `.0`), else the parameter type itself.
fn validation_target_type(ty: &syn::Type) -> syn::Type {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            if matches!(ident.as_str(), "Json" | "Query" | "Path" | "Form") {
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

/// The `RouteInfo::rejection_kinds` expression: the static kinds, plus what
/// only the compiled program knows — the struct-level identity
/// (`HAS_STRUCT_IDENTITY`, skipped by `#[anonymous]`), the locations of
/// `#[derive(Params)]` fields (`__params`, bound by the caller), whether a
/// parameter type implements `garde::Validate` (autoref probe, the same
/// decision `__maybe_validate` makes), and the kinds a custom body extractor
/// declares (`__body`, bound by the caller). Deduplicated, order-insensitive.
fn rejection_kinds_expr(
    static_kinds: &[&str],
    anonymous: bool,
    extracted: &[syn::PatType],
    meta_mod: &syn::Ident,
) -> TokenStream {
    let krate = r2e_core_path();
    let statics: Vec<TokenStream> = static_kinds
        .iter()
        .map(|k| {
            let ident = format_ident!("{}", k);
            quote! { #krate::RejectionKind::#ident }
        })
        .collect();
    let struct_identity = if anonymous {
        quote! { false }
    } else {
        quote! { #meta_mod::HAS_STRUCT_IDENTITY }
    };
    let validate_probes: Vec<TokenStream> = extracted
        .iter()
        .map(|pt| {
            let ty = validation_target_type(&pt.ty);
            quote! {
                {
                    struct __ValidateProbe<T>(::core::marker::PhantomData<T>);
                    trait __NoValidate {
                        fn __validates(&self) -> bool { false }
                    }
                    impl<T> __NoValidate for &__ValidateProbe<T> {}
                    impl<T: #krate::web::validation::Validate> __ValidateProbe<T>
                    where
                        T::Context: Default,
                    {
                        fn __validates(&self) -> bool { true }
                    }
                    use __NoValidate as _;
                    if (&__ValidateProbe::<#ty>(::core::marker::PhantomData)).__validates() {
                        __kinds.push(#krate::RejectionKind::Validation);
                    }
                }
            }
        })
        .collect();

    quote! {
        {
            let mut __kinds: Vec<#krate::RejectionKind> = vec![#(#statics),*];
            for __p in &__params {
                __kinds.push(match __p.location {
                    #krate::di::meta::ParamLocation::Path => #krate::RejectionKind::InvalidPath,
                    #krate::di::meta::ParamLocation::Query => #krate::RejectionKind::InvalidQuery,
                    #krate::di::meta::ParamLocation::Header => #krate::RejectionKind::InvalidHeader,
                });
            }
            if #struct_identity {
                __kinds.push(#krate::RejectionKind::Unauthenticated);
            }
            #(#validate_probes)*
            if let Some(__b) = &__body {
                __kinds.extend(__b.rejection_kinds.iter().copied());
            }
            let mut __seen = ::std::collections::HashSet::new();
            __kinds.retain(|__k| __seen.insert(*__k));
            __kinds
        }
    }
}

/// The `RouteInfo.params` expression for a handler signature.
///
/// Two sources, folded into one deduplicated vec: `Path(name): Path<T>`
/// parameters contribute a path `ParamInfo` literal, and every parameter type
/// is probed for `ParamsMetadata` so `#[derive(Params)]` structs publish their
/// own fields.
///
/// Shared by verb routes and `#[sse]` / `#[ws]` routes — a streaming route
/// extracts its parameters exactly like a verb route, so it documents them the
/// same way. `skip_param` (an index over the *typed* parameters, `&self`
/// excluded) drops the WS socket parameter, which the upgrade supplies rather
/// than an extractor.
fn params_expr(
    sig: &syn::Signature,
    skip_param: Option<usize>,
    krate: &TokenStream,
) -> TokenStream {
    let params: Vec<&syn::PatType> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(pt),
            // skip &self
            syn::FnArg::Receiver(_) => None,
        })
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip_param)
        .map(|(_, pt)| pt)
        .collect();

    let path_params = extract_path_params(&params, krate);

    // Autoref specialization: for each handler param type, probe for ParamsMetadata.
    // Types implementing ParamsMetadata return their param infos; others return empty vec.
    // Wrapper types like `Query<T>` / `Path<T>` are unwrapped first, since `T` is
    // where `ParamsMetadata` would be implemented.
    let probe_blocks: Vec<TokenStream> = params
        .iter()
        .map(|pt| {
            let ty = unwrap_extractor_inner(&pt.ty);
            quote! {
                {
                    let __probe = #krate::web::params::__ParamMetaProbe::<#ty>(::core::marker::PhantomData);
                    use #krate::web::params::__NoParamsMeta as _;
                    __p.extend((&__probe).param_infos());
                }
            }
        })
        .collect();

    quote! {
        {
            let mut __p: Vec<#krate::di::meta::ParamInfo> = vec![#(#path_params),*];
            #(#probe_blocks)*
            // Deduplicate params by (name, location) — possible when
            // a Params struct includes #[param(path)] fields alongside Path<T>.
            {
                let mut seen = ::std::collections::HashSet::new();
                __p.retain(|p| seen.insert((p.name.clone(), format!("{:?}", p.location))));
            }
            __p
        }
    }
}

/// Extract path parameters from the handler's typed parameters.
fn extract_path_params(params: &[&syn::PatType], krate: &TokenStream) -> Vec<TokenStream> {
    params
        .iter()
        .filter_map(|pt| {
            let ty = &pt.ty;
            if type_last_segment_is(ty, "Path") {
                if let syn::Pat::TupleStruct(ts) = pt.pat.as_ref() {
                    if let Some(elem) = ts.elems.first() {
                        let param_name = quote!(#elem).to_string();
                        let param_type = infer_path_param_openapi_type(&pt.ty);
                        return Some(quote! {
                            #krate::di::meta::ParamInfo {
                                name: #param_name.to_string(),
                                location: #krate::di::meta::ParamLocation::Path,
                                param_type: #param_type.to_string(),
                                required: true,
                            }
                        });
                    }
                }
            }
            None
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
fn default_status_for_method(method: &crate::model::route::HttpMethod) -> u16 {
    match method {
        crate::model::route::HttpMethod::Get => 200,
        crate::model::route::HttpMethod::Post => 201,
        crate::model::route::HttpMethod::Put => 200,
        crate::model::route::HttpMethod::Delete => 204,
        crate::model::route::HttpMethod::Patch => 200,
        crate::model::route::HttpMethod::Any => 200,
    }
}

/// Whether a route path contains an axum `{*wildcard}` segment. Such paths
/// are not valid OpenAPI path templates, so the route is excluded from the spec.
fn is_wildcard_path(path: &str) -> bool {
    path.contains("{*")
}

/// The success-response classification of a route for `RouteInfo`.
struct ResponseInfo {
    /// Expression of type `Option<Vec<ResponseContent>>`: `Some(contents)`
    /// when the return type implements `ResponseBodySchema` (an empty vec is
    /// an intentional no-body return), `None` when it is unmapped.
    probe: TokenStream,
    /// The readable type name reported as `response_unmapped` when the probe
    /// yields `None`.
    unmapped: Option<String>,
}

/// Classify the route's successful response.
///
/// * `#[returns(T)]`: `T` is probed for `ResponseBodySchema`, then `Json<T>`
///   is — so `#[returns(User)]` on an `impl IntoResponse` handler documents
///   `application/json` with `User`'s schema when `User: JsonSchema`.
/// * No return type → no body.
/// * `impl Trait` → unmapped (nothing to probe).
/// * Otherwise the return type (its `Result<T, E>` unwrapped for the warning
///   name; the probe works on the full type since `Result<T, E>` delegates to
///   `T`) is probed. A type containing `impl Trait` (`Sse<impl Stream<..>>`)
///   cannot be named in a turbofish: `Sse<..>` documents an event stream, any
///   other such type is unmapped.
fn response_info(rm: &crate::model::types::RouteMethod) -> ResponseInfo {
    let krate = r2e_core_path();
    if let Some(returns_ty) = &rm.decorators.returns_type {
        let direct = response_probe_tokens(returns_ty);
        let json_ty: syn::Type = syn::parse_quote!(#krate::http::Json<#returns_ty>);
        let as_json = response_probe_tokens(&json_ty);
        return ResponseInfo {
            probe: quote! { #direct.or_else(|| #as_json) },
            unmapped: Some(readable_type(returns_ty)),
        };
    }
    let ret_ty = match &rm.fn_item.sig.output {
        syn::ReturnType::Default => {
            return ResponseInfo {
                probe: quote! { Some(Vec::new()) },
                unmapped: None,
            }
        }
        syn::ReturnType::Type(_, ty) => ty.as_ref(),
    };
    if matches!(ret_ty, syn::Type::ImplTrait(_)) {
        return ResponseInfo {
            probe: quote! { None },
            unmapped: Some(readable_type(ret_ty)),
        };
    }
    let unwrapped = unwrap_result_type(ret_ty);
    if contains_impl_trait(&quote!(#unwrapped)) {
        if type_last_segment_is(unwrapped, "Sse") {
            return ResponseInfo {
                probe: quote! { Some(vec![#krate::di::meta::ResponseContent::event_stream(None)]) },
                unmapped: None,
            };
        }
        return ResponseInfo {
            probe: quote! { None },
            unmapped: Some(readable_type(unwrapped)),
        };
    }
    ResponseInfo {
        probe: response_probe_tokens(ret_ty),
        unmapped: Some(readable_type(unwrapped)),
    }
}

/// An autoref-specialization probe of `ty` for `ResponseBodySchema`:
/// `Some(T::response_contents())` when implemented, `None` otherwise.
fn response_probe_tokens(ty: &syn::Type) -> TokenStream {
    let krate = r2e_core_path();
    quote! {
        {
            struct __ResponseProbe<T>(::core::marker::PhantomData<T>);
            trait __NoResponseSchema {
                fn __contents(&self) -> Option<Vec<#krate::di::meta::ResponseContent>> { None }
            }
            impl<T> __NoResponseSchema for &__ResponseProbe<T> {}
            impl<T: #krate::di::meta::ResponseBodySchema> __ResponseProbe<T> {
                fn __contents(&self) -> Option<Vec<#krate::di::meta::ResponseContent>> {
                    Some(<T as #krate::di::meta::ResponseBodySchema>::response_contents())
                }
            }
            use __NoResponseSchema as _;
            (&__ResponseProbe::<#ty>(::core::marker::PhantomData)).__contents()
        }
    }
}

/// Render a `syn::Type` as a readable Rust type name for warning messages,
/// collapsing the spaces `quote!` inserts around `<`, `>`, `,`, and `::`.
fn readable_type(ty: &syn::Type) -> String {
    quote!(#ty)
        .to_string()
        .replace(" <", "<")
        .replace("< ", "<")
        .replace(" >", ">")
        .replace("> ", ">")
        .replace(" ,", ",")
        .replace(" ::", "::")
        .replace(":: ", "::")
}

fn contains_impl_trait(tokens: &TokenStream) -> bool {
    tokens.clone().into_iter().any(|tt| match tt {
        proc_macro2::TokenTree::Ident(ident) => ident == "impl",
        proc_macro2::TokenTree::Group(group) => contains_impl_trait(&group.stream()),
        _ => false,
    })
}

/// The request-body classification of a route.
struct BodyInfo {
    /// The body extractor type (its `Option<..>` unwrapped), probed for
    /// `RequestBodySchema` at runtime. `None` when no parameter reads the body.
    ty: Option<syn::Type>,
    /// `false` for an `Option<..>`-wrapped extractor.
    required: bool,
}

impl BodyInfo {
    const NONE: Self = Self { ty: None, required: true };

    /// `Some("<readable type>")` token when a body-position parameter exists,
    /// reported as `request_body_unmapped` if its probe yields `None`.
    fn unmapped_token(&self) -> TokenStream {
        match &self.ty {
            Some(ty) => {
                let name = readable_type(ty);
                quote! { Some(#name.to_string()) }
            }
            None => quote! { None },
        }
    }

    /// The `__body` binding: an autoref probe of the body type for
    /// `RequestBodySchema` — `Some(..)` when the type implements it (the
    /// framework's `Json`, `Bytes`, `String`, `Form`, `Multipart`,
    /// `TypedMultipart` and any custom extractor), `None` otherwise.
    fn probe_expr(&self) -> TokenStream {
        let Some(ty) = &self.ty else {
            return quote! { None };
        };
        let krate = r2e_core_path();
        quote! {
            {
                struct __BodyProbe<T>(::core::marker::PhantomData<T>);
                trait __NoBodySchema {
                    fn __body(&self) -> Option<#krate::di::meta::__BodyProbeResult> { None }
                }
                impl<T> __NoBodySchema for &__BodyProbe<T> {}
                impl<T: #krate::di::meta::RequestBodySchema> __BodyProbe<T> {
                    fn __body(&self) -> Option<#krate::di::meta::__BodyProbeResult> {
                        Some(#krate::di::meta::__BodyProbeResult {
                            content_type: <T as #krate::di::meta::RequestBodySchema>::content_type(),
                            schema: <T as #krate::di::meta::RequestBodySchema>::body_schema(),
                            rejection_kinds: <T as #krate::di::meta::RequestBodySchema>::rejection_kinds(),
                        })
                    }
                }
                use __NoBodySchema as _;
                (&__BodyProbe::<#ty>(::core::marker::PhantomData)).__body()
            }
        }
    }
}

/// Classify the route's request body: the last extracted parameter when the
/// entry function reads it through `FromRequest` — it is the only one that can
/// be a body extractor. `Option<X>` unwraps to `X` with `required: false`.
/// Parameter types that never read the body are skipped so the generated
/// metadata stays free of pointless probes; `Form<T>` on GET reads the query
/// string, not the body.
fn extract_body_info(
    extracted: &[syn::PatType],
    last_consumes_body: bool,
    method: &crate::model::route::HttpMethod,
) -> BodyInfo {
    if !last_consumes_body {
        return BodyInfo::NONE;
    }
    let Some(last) = extracted.last() else {
        return BodyInfo::NONE;
    };
    let (ty, required) = match unwrap_option_type(&last.ty) {
        Some(inner) => (inner.clone(), false),
        None => ((*last.ty).clone(), true),
    };
    if is_known_non_body_type(&ty) {
        return BodyInfo::NONE;
    }
    if type_last_segment_is(&ty, "Form") && matches!(method, crate::model::route::HttpMethod::Get) {
        return BodyInfo::NONE;
    }
    BodyInfo {
        ty: Some(ty),
        required,
    }
}

/// Parameter types that never read the body.
fn is_known_non_body_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return matches!(
                segment.ident.to_string().as_str(),
                "Path" | "Query" | "HeaderMap" | "Method" | "Uri" | "Version" | "Extension"
                    | "ConnectInfo" | "State"
            );
        }
    }
    matches!(ty, syn::Type::Reference(_))
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
    // Only the interceptor fields matter off-request: controller-level guards
    // are HTTP-only, so a set that exists for guards alone must not promote
    // scheduled bodies or add the container's `__ctrl` field.
    let ctrl_for_transverse = ctrl_set
        .as_ref()
        .is_some_and(|s| !s.intercept_fields.is_empty())
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
            let set = ctrl_set
                .as_ref()
                .expect("ctrl_for_transverse implies ctrl_set");
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
        let methods: syn::Result<Vec<ScheduledSourceMethod>> = def
            .scheduled_methods
            .iter()
            .zip(sched_sets.iter())
            .map(|(sm, set)| {
                Ok(ScheduledSourceMethod {
                    fn_name: sm.fn_item.sig.ident.clone(),
                    config: sm.config.clone(),
                    // Intercepted methods self-intercept in their dispatch wrapper
                    // (sync sources promoted to `async fn`), so the emitted call is
                    // awaited when the source is async OR it runs method-level
                    // interceptors OR it runs the shared controller-level ones.
                    emitted_async: sm.fn_item.sig.asyncness.is_some()
                        || set.is_some()
                        || ctrl_for_transverse,
                    // `skip_if` predicates live among the impl block's plain
                    // methods (routes/consumers/scheduled are classified away
                    // from `other_methods` during parsing).
                    skip: crate::codegen::scheduled::resolve_skip_if(
                        &sm.config,
                        def.other_methods.iter(),
                    )?,
                })
            })
            .collect();
        match methods {
            // An unresolvable `skip_if` surfaces as a compile_error at module
            // scope (this generator is infallible; the trait default applies).
            Err(e) => module_items.push(e.to_compile_error()),
            Ok(methods) => {
                let task_defs =
                    transverse::scheduled_task_defs(&quote! { __core }, &owner_name, &methods);
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
        }
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
        let calls = transverse::pre_destroy_calls(
            &quote! { __self },
            &owner_name,
            &def.pre_destroy_methods,
        );
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

    // `#[on_start]` startup observers. Like `pre_destroy`, controller cores are
    // not `Clone` and so cannot impl the `OnStart` trait; each hook binds a
    // clone of the core `Arc` instead. The builder merges these with the bean
    // hooks and awaits them in `order` at startup — an `Err` aborts boot.
    if !def.on_start_methods.is_empty() {
        let pushes = transverse::on_start_pushes(
            &quote! { ::std::sync::Arc::clone(&__core) },
            &def.on_start_methods,
        );
        controller_fns.push(quote! {
            fn on_start(
                __core: ::std::sync::Arc<Self>,
            ) -> Vec<(i32, #krate::beans::OnStartHook)> {
                let mut __r2e_hooks: Vec<(i32, #krate::beans::OnStartHook)> = Vec::new();
                #(#pushes)*
                __r2e_hooks
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
    let krate = r2e_core_path();
    def.sse_methods
        .iter()
        .map(|sm| {
            let roles = streaming_effective_roles(def, &sm.decorators);
            // The same params the SSE entry fn extracts (its last may read
            // the body) — see `handlers::RequestParams`.
            let request = super::handlers::RequestParams::sse(sm);
            let extracted = request.owned();
            let body = extract_body_info(
                &extracted,
                request.last_consumes_body,
                &crate::model::route::HttpMethod::Get,
            );
            let kinds = static_rejection_kinds(
                def,
                &sm.decorators,
                sm.identity_param.as_ref(),
                !roles.is_empty(),
                &extracted,
                request.last_consumes_body,
                &crate::model::route::HttpMethod::Get,
            );
            emit_streaming_route_info(
                name,
                meta_mod,
                &sm.path,
                &sm.fn_item.sig,
                &roles,
                &kinds,
                &extracted,
                body.probe_expr(),
                body.required,
                body.unmapped_token(),
                quote! { vec![#krate::di::meta::ResponseContent::event_stream(None)] },
                sm.decorators.anonymous,
                &sm.fn_item.attrs,
                None,
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
            let roles = streaming_effective_roles(def, &wm.decorators);
            // The same params the WS entry fn extracts (none reads the body).
            let request = super::handlers::RequestParams::ws(wm);
            let extracted = request.owned();
            let kinds = static_rejection_kinds(
                def,
                &wm.decorators,
                wm.identity_param.as_ref(),
                !roles.is_empty(),
                &extracted,
                request.last_consumes_body,
                &crate::model::route::HttpMethod::Get,
            );
            emit_streaming_route_info(
                name,
                meta_mod,
                &wm.path,
                &wm.fn_item.sig,
                &roles,
                &kinds,
                &extracted,
                quote! { None },
                true,
                quote! { None },
                quote! { Vec::new() },
                wm.decorators.anonymous,
                &wm.fn_item.attrs,
                // The socket itself comes from the upgrade, not from an
                // extractor — it is not a documentable parameter.
                wm.ws_param.as_ref().map(|p| p.index),
                "WebSocket endpoint",
            )
        })
        .collect()
}

/// Effective (method + controller) role strings for a streaming route's
/// metadata. Controller-level decorators apply on the same rule as routes:
/// every non-`#[anonymous]` endpoint (anonymous opts out of the controller's
/// post-auth checks).
fn streaming_effective_roles(
    def: &RoutesImplDef,
    decorators: &crate::model::types::MethodDecorators,
) -> Vec<String> {
    let ctrl = &def.controller_decorators;
    let mut roles: Vec<String> = decorators
        .roles
        .iter()
        .chain(decorators.all_roles.iter())
        .cloned()
        .collect();
    if !decorators.anonymous {
        roles.extend(ctrl.roles.iter().chain(ctrl.all_roles.iter()).cloned());
    }
    roles
}

/// Emit a `RouteInfo` literal for SSE / WS routes.
///
/// Both emit a `GET` with empty body/response and a 200 status; they differ
/// only in the fallback summary and in which parameter the socket occupies.
/// Keeping this in one place makes adding a new streaming route kind (or a new
/// `RouteInfo` field) a single-edit affair.
///
/// Summary and description come from the method's doc comment, exactly as for
/// a verb route (`generate_route_metadata`), so moving a documented `#[get]`
/// to `#[sse]` keeps its OpenAPI prose. `fallback_summary` — "SSE stream" /
/// "WebSocket endpoint" — only applies to an undocumented method.
///
/// Parameters go through the same `params_expr` as a verb route: a streaming
/// method extracts `Path<T>` / `#[derive(Params)]` arguments like any other
/// handler, so it documents them the same way. `ws_param` is the index of the
/// socket parameter, excluded from that list.
///
/// `static_kinds` are the macro-time rejection kinds, computed over the same
/// extracted parameters as the entry fn (`handlers::RequestParams`);
/// `extracted` feeds the runtime garde probes and `body_probe` a custom body
/// extractor's declared kinds (SSE only — a WS endpoint reads no body), so a
/// `Query<T>` or a validated parameter is documented exactly as on a verb
/// route. Its return type is a stream, never an envelope, so `error_schema` is
/// `None` (application projection).
#[allow(clippy::too_many_arguments)]
fn emit_streaming_route_info(
    controller_name: &syn::Ident,
    meta_mod: &syn::Ident,
    path: &str,
    sig: &syn::Signature,
    roles: &[String],
    static_kinds: &[&str],
    extracted: &[syn::PatType],
    body_probe: TokenStream,
    body_required: bool,
    body_unmapped: TokenStream,
    response_contents: TokenStream,
    anonymous: bool,
    attrs: &[syn::Attribute],
    ws_param: Option<usize>,
    fallback_summary: &str,
) -> TokenStream {
    let krate = r2e_core_path();
    let op_id = format!("{}_{}", controller_name, sig.ident);
    let params_expr = params_expr(sig, ws_param, &krate);
    let roles_tokens: Vec<_> = roles.iter().map(|r| quote! { #r.to_string() }).collect();
    let kinds_expr = rejection_kinds_expr(static_kinds, anonymous, extracted, meta_mod);

    let (doc_summary, doc_description) = crate::extract::route::extract_doc_comments(attrs);
    let summary = doc_summary.unwrap_or_else(|| fallback_summary.to_string());
    let description_token = match doc_description {
        Some(d) => quote! { Some(#d.to_string()) },
        None => quote! { None },
    };

    quote! {
        {
            let __params: Vec<#krate::di::meta::ParamInfo> = #params_expr;
            let __body: Option<#krate::di::meta::__BodyProbeResult> = #body_probe;
            let __kinds: Vec<#krate::RejectionKind> = #kinds_expr;
            #krate::di::meta::RouteInfo {
                path: match #meta_mod::PATH_PREFIX {
                    Some(__prefix) => format!("{}{}", __prefix, #path),
                    None => #path.to_string(),
                },
                method: "GET".to_string(),
                operation_id: #op_id.to_string(),
                summary: Some(#summary.to_string()),
                description: #description_token,
                request_body_unmapped: if __body.is_none() { #body_unmapped } else { None },
                request_body: __body.map(|__b| #krate::di::meta::RequestBody {
                    content_type: __b.content_type.to_string(),
                    schema: __b.schema,
                    required: #body_required,
                }),
                response_status: 200,
                response_unmapped: None,
                response_contents: #response_contents,
                params: __params,
                roles: vec![#(#roles_tokens),*],
                tag: Some(#meta_mod::OPENAPI_TAG.to_string()),
                deprecated: false,
                rejection_kinds: __kinds,
                error_schema: None,
            }
        }
    }
}

// ── Application-scoped route registrations ─────────────────────────────
//
// These produce the `.route(path, METHOD(closure))` fragments registered
// inside the state-aware application-controller closure. Each fragment
// captures the controller `Arc` (and the prebuilt decorator sets) once and
// forwards to the entry function emitted by `handlers.rs`, which runs the
// whole request pipeline — pre-auth guards included.

fn generate_route_registrations(def: &RoutesImplDef) -> Vec<TokenStream> {
    let krate = r2e_core_path();
    def.route_methods
        .iter()
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
