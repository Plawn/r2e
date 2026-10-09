//! Entry-function generation for route / SSE / WS methods.
//!
//! Every endpoint gets ONE generated entry function, registered through a
//! `(State<S>, Request)` closure. The entry function owns the whole request
//! pipeline in a fixed order — pre-auth guards, request-scoped data (identity +
//! `#[inject(request)]`), the param-level identity, guards, the remaining
//! extractors (body last), garde validation, `#[managed]` acquisition, the
//! interceptor chain around the method call, managed finalisation — and every
//! failure of that pipeline is a typed [`Rejection`] projected **once** through
//! the route's error envelope (see `r2e_core::error::projection`).

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::model::types::*;
use crate::parsing::routes_parsing::RoutesImplDef;
use crate::util::crate_path::r2e_core_path;

/// Generate all entry functions for a controller.
pub fn generate_handlers(def: &RoutesImplDef) -> TokenStream {
    let route_handlers: Vec<_> = def
        .route_methods
        .iter()
        .map(|rm| generate_single_handler(def, rm))
        .collect();

    let sse_handlers: Vec<_> = def
        .sse_methods
        .iter()
        .map(|sm| generate_sse_handler(def, sm))
        .collect();

    let ws_handlers: Vec<_> = def
        .ws_methods
        .iter()
        .map(|wm| generate_ws_handler(def, wm))
        .collect();

    quote! {
        #(#route_handlers)*
        #(#sse_handlers)*
        #(#ws_handlers)*
    }
}

fn invocation_ident_for(controller: &syn::Ident, method: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_invoke_{}_{}", controller, method)
}

/// The post-upgrade session body of a `#[ws]` method (runs on the socket).
fn ws_session_ident_for(controller: &syn::Ident, method: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_session_{}_{}", controller, method)
}

/// The generic state ident shared by all state-generic generated items. Free
/// generated fns declare it themselves; items inside the `Controller` impl use
/// the impl's parameter of the same name.
pub(super) fn state_generic() -> syn::Ident {
    format_ident!("__R2eS")
}

/// The marker generic carried by the request-data extractor (a tuple of
/// per-field `FromRequestPartsVia` markers, opaque to `#[routes]`).
pub(super) fn data_marker() -> syn::Ident {
    format_ident!("__R2eMd")
}

/// The extraction marker for a route's param-level `#[inject(identity)]`.
/// Pascal-cased so the generated type parameter doesn't trip
/// `non_camel_case_types` in user crates.
pub(super) fn identity_marker_for(method: &syn::Ident) -> syn::Ident {
    format_ident!(
        "__R2eMp{}",
        crate::util::type_utils::to_pascal_case(&method.to_string())
    )
}

/// Bounds placed on the generic state by every generated item that touches it:
/// axum's `Router` requirements plus `BeanLookup`, the fixed vocabulary through
/// which guards, interceptors, managed resources and the error projection pull
/// beans from the state.
pub(super) fn state_bounds(krate: &TokenStream) -> TokenStream {
    quote! { Clone + Send + Sync + 'static + #krate::BeanLookup }
}

/// The generated request façade type for a controller. Route/SSE/WS methods are
/// emitted on `impl __R2eRequest_<Name>`; handler invocation runs on a borrow of
/// it. Application/config fields and core helpers are reached through its
/// `Deref<Target = Core>`.
fn facade_ident_for(controller: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eRequest_{}", controller)
}

/// The receiver type a route method is invoked on: the controller **core** for
/// `#[anonymous]` routes (no request-scoped extraction), the request façade
/// otherwise. Single decision point for all route kinds.
fn receiver_ty_for(anonymous: bool, controller: &syn::Ident) -> TokenStream {
    if anonymous {
        quote! { #controller }
    } else {
        let facade = facade_ident_for(controller);
        quote! { #facade }
    }
}

/// The generated request-data extractor type for a controller. Carries the
/// request-scoped values (identity + `#[inject(request)]`) and is bound into the
/// façade alongside the captured core `Arc`.
fn request_data_ident_for(controller: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eRequestData_{}", controller)
}

/// The controller name as seen by guards: **module-qualified**, e.g.
/// `my_app::admin::UsersController`.
///
/// `GuardContext::controller_name` / `PreAuthGuardContext::controller_name` feed
/// identity-scoped guard state — rate-limit bucket keys above all — so the bare
/// type identifier is not enough: `public::UsersController::list` and
/// `admin::UsersController::list` would collide and share one budget. Prefixing
/// with `module_path!()` (expanded in the controller's own crate and module)
/// makes the name route-unique at zero macro cost — the whole thing is a
/// compile-time `&'static str` literal.
pub(super) fn qualified_controller_name(controller: &syn::Ident) -> TokenStream {
    let bare = controller.to_string();
    quote! { ::core::concat!(::core::module_path!(), "::", #bare) }
}

/// Walk a method signature once and collect its typed params with indices,
/// dropping the `&self` receiver. Shared by HTTP / SSE / WS handler codegen.
fn extract_sig_params(sig: &syn::Signature) -> Vec<(usize, &syn::PatType)> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => Some(pat_type),
            syn::FnArg::Receiver(_) => None,
        })
        .enumerate()
        .collect()
}

/// Check if a type is a known Axum wrapper (Json, Query, Path, Form).
fn is_wrapper_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident = segment.ident.to_string();
            return matches!(ident.as_str(), "Json" | "Query" | "Path" | "Form");
        }
    }
    false
}

// ── Error projection ─────────────────────────────────────────────────────

/// How a route projects the [`Rejection`]s of its request pipeline.
enum Projection {
    /// Probe the handler's declared return type: `Result<T, E>` with an
    /// envelope `E` projects through `E`; anything else falls back to the
    /// application projection. Resolved by autoref specialization at
    /// compile time, so the fallback costs nothing where it applies.
    Probe(syn::Type),
    /// The application projection (`ErrorProjector` bean, else `HttpError`).
    /// Used when the return type cannot be named as a type argument (`impl
    /// Trait`), for SSE/WS endpoints, and for infallible handlers.
    Default,
}

impl Projection {
    /// From a handler signature: its output type when it is nameable.
    fn for_signature(sig: &syn::Signature) -> Self {
        match &sig.output {
            syn::ReturnType::Default => Self::Default,
            syn::ReturnType::Type(_, ty) => {
                if contains_impl_trait(ty) {
                    Self::Default
                } else {
                    Self::Probe((**ty).clone())
                }
            }
        }
    }

    /// A `return <projected response>;` statement for a `Rejection` expression.
    /// `state` is an expression of type `&S` (`S: BeanLookup`).
    fn return_stmt(&self, krate: &TokenStream, rejection: TokenStream, state: &TokenStream) -> TokenStream {
        match self {
            Self::Probe(ty) => quote! {
                {
                    use #krate::error::projection::ProjectEnvelope as _;
                    use #krate::error::projection::ProjectFallback as _;
                    return (&#krate::error::projection::ProjectionProbe::<#ty>::new())
                        .project(#rejection, #state);
                }
            },
            Self::Default => quote! {
                {
                    return #krate::error::projection::project_default(#rejection, #state);
                }
            },
        }
    }
}

fn contains_impl_trait(ty: &syn::Type) -> bool {
    struct Finder(bool);
    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_type_impl_trait(&mut self, _: &'ast syn::TypeImplTrait) {
            self.0 = true;
        }
    }
    let mut finder = Finder(false);
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.0
}

// ── Decorator plan ───────────────────────────────────────────────────────

/// Everything an endpoint's entry function and its registration closure must
/// agree on about decorator sets: which sets exist (after spec-type
/// degradation), and which fields of each apply to this endpoint.
///
/// Computed by ONE function ([`plan_endpoint`]) from the same inputs on both
/// sides, so a spec-type error can never produce an arity mismatch between
/// the closure call and the entry function's signature.
struct EndpointPlan {
    /// Per-method set (guards + method-level interceptors) items; carries the
    /// `compile_error!` when a spec type is not inferable.
    deco_items: TokenStream,
    predeco_items: TokenStream,
    deco_set: Option<super::decorators::DecoSet>,
    predeco_set: Option<super::decorators::DecoSet>,
    /// The shared controller-level set, when this endpoint captures it.
    ctrl_set: Option<super::decorators::CtrlDecoSet>,
    anonymous: bool,
}

/// Build the decorator plan of one endpoint.
///
/// `intercepts_apply` is true for HTTP routes only — SSE/WS endpoints never run
/// the interceptor chain, so they capture the controller-level set only for
/// its guard / pre-guard fields.
fn plan_endpoint(
    def: &RoutesImplDef,
    fn_ident: &syn::Ident,
    decorators: &MethodDecorators,
    intercepts_apply: bool,
    path: &str,
    sig: &syn::Signature,
) -> EndpointPlan {
    let krate = r2e_core_path();
    let intercept_exprs: Vec<&syn::Expr> = if intercepts_apply {
        decorators.intercept_fns.iter().collect()
    } else {
        Vec::new()
    };
    let path_module = generate_path_param_module(path, sig, &krate);
    let (deco_items, deco_set) = super::decorators::generate_deco_items(
        def,
        fn_ident,
        &decorators.guard_fns,
        &intercept_exprs,
        path_module,
    );
    let (predeco_items, predeco_set) =
        super::decorators::generate_predeco_items(def, fn_ident, decorators);

    // Degradation: when any spec (method-level guard/interceptor, or any
    // controller-level decorator) is not inferable, drop every set so the
    // only error the user sees is the spec-type one. The compile_error comes
    // from this method's own deco/predeco items for method-level specs and
    // from `generate_ctrl_deco_items` (once per controller) for
    // controller-level specs — degradation is never silent.
    let specs_ok = super::decorators::specs_ok_with_ctrl(
        def,
        decorators
            .guard_fns
            .iter()
            .chain(intercept_exprs.iter().copied()),
    );
    let ctrl_set = super::decorators::ctrl_deco_set(def)
        .filter(|_| specs_ok)
        .filter(|s| {
            intercepts_apply || !s.guard_fields.is_empty() || !s.pre_guard_fields.is_empty()
        });

    EndpointPlan {
        deco_items,
        predeco_items,
        deco_set: deco_set.filter(|_| specs_ok),
        predeco_set: predeco_set.filter(|_| specs_ok),
        ctrl_set,
        anonymous: decorators.anonymous,
    }
}

impl EndpointPlan {
    /// Controller-level post-auth guards apply to every route EXCEPT
    /// `#[anonymous]` ones: the marker opts the route out of the controller's
    /// auth surface, so identity-driven controller guards must not fire there
    /// (pre-guards and interceptors still do).
    fn ctrl_guard_fields(&self) -> &[syn::Ident] {
        match &self.ctrl_set {
            Some(s) if !self.anonymous => &s.guard_fields,
            _ => &[],
        }
    }

    fn ctrl_pre_fields(&self) -> &[syn::Ident] {
        self.ctrl_set
            .as_ref()
            .map(|s| s.pre_guard_fields.as_slice())
            .unwrap_or(&[])
    }

    fn ctrl_intercept_fields(&self) -> &[syn::Ident] {
        self.ctrl_set
            .as_ref()
            .map(|s| s.intercept_fields.as_slice())
            .unwrap_or(&[])
    }

    fn method_guard_fields(&self) -> &[syn::Ident] {
        self.deco_set
            .as_ref()
            .map(|s| s.guard_fields.as_slice())
            .unwrap_or(&[])
    }

    fn method_intercept_fields(&self) -> &[syn::Ident] {
        self.deco_set
            .as_ref()
            .map(|s| s.intercept_fields.as_slice())
            .unwrap_or(&[])
    }

    fn pre_fields(&self) -> &[syn::Ident] {
        self.predeco_set
            .as_ref()
            .map(|s| s.guard_fields.as_slice())
            .unwrap_or(&[])
    }

    /// Any post-auth guard (controller- or method-level) runs on this endpoint.
    fn has_guards(&self) -> bool {
        !self.ctrl_guard_fields().is_empty() || !self.method_guard_fields().is_empty()
    }

    fn has_intercepts(&self) -> bool {
        !self.ctrl_intercept_fields().is_empty() || !self.method_intercept_fields().is_empty()
    }

    /// Combined interceptor refs, impl-level (shared) outermost then
    /// method-level — unchanged execution order.
    fn interceptor_refs(&self) -> Vec<TokenStream> {
        let mut refs: Vec<TokenStream> = Vec::new();
        for f in self.ctrl_intercept_fields() {
            refs.push(quote! { &__ctrl_deco.#f });
        }
        for f in self.method_intercept_fields() {
            refs.push(quote! { &__deco.#f });
        }
        refs
    }

    /// Decorator-set parameters of the entry function (after `__req`).
    fn entry_params(&self) -> Vec<TokenStream> {
        let mut params = Vec::new();
        if let Some(cs) = &self.ctrl_set {
            let ty = &cs.struct_ident;
            params.push(quote! { __ctrl_deco: &#ty });
        }
        if let Some(ps) = &self.predeco_set {
            let ty = ps.ty();
            params.push(quote! { __pre_deco: &#ty });
        }
        if let Some(ds) = &self.deco_set {
            let ty = ds.ty();
            params.push(quote! { __deco: &#ty });
        }
        params
    }

    /// Registration-time construction of the captured sets (closure side).
    /// The method's sets are built once here, from the resolved graph, and
    /// captured as one `Arc` each; the shared controller-level set
    /// (`__r2e_ctrl_deco`, built once in the router body) is captured by an
    /// `Arc` clone so every route shares one instance.
    fn capture_setup(&self) -> TokenStream {
        let ctrl = self.ctrl_set.as_ref().map(|_| {
            quote! { let __ctrl_deco_capture = ::std::sync::Arc::clone(&__r2e_ctrl_deco); }
        });
        let pre = self.predeco_set.as_ref().map(|s| {
            let ctor = &s.ctor_ident;
            quote! { let __pre_deco_capture = ::std::sync::Arc::new(#ctor(__ctx)); }
        });
        let deco = self.deco_set.as_ref().map(|s| {
            let ctor = &s.ctor_ident;
            quote! { let __deco_capture = ::std::sync::Arc::new(#ctor(__ctx)); }
        });
        quote! { #ctrl #pre #deco }
    }

    /// The matching call arguments (same order as [`Self::entry_params`]).
    fn capture_args(&self) -> Vec<TokenStream> {
        let mut args = Vec::new();
        if self.ctrl_set.is_some() {
            args.push(quote! { &__ctrl_deco_capture });
        }
        if self.predeco_set.is_some() {
            args.push(quote! { &__pre_deco_capture });
        }
        if self.deco_set.is_some() {
            args.push(quote! { &__deco_capture });
        }
        args
    }
}

// ── Path-parameter symbols (`mod path { const id: PathParam<T> }`) ───────

struct PathParamSymbol {
    ident: syn::Ident,
    name: String,
    ty: syn::Type,
}

/// Extract `{name}` parameters from an Axum-style route path.
pub(super) fn extract_route_path_param_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = path;

    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let raw = &after_open[..close];
        let name = raw
            .split(':')
            .next()
            .unwrap_or(raw)
            .trim()
            .trim_start_matches('*');
        if !name.is_empty() {
            names.push(name.to_string());
        }
        rest = &after_open[close + 1..];
    }

    names
}

fn path_wrapper_inner_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Path" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty.clone()),
        _ => None,
    })
}

fn flatten_path_inner_types(ty: &syn::Type) -> Vec<syn::Type> {
    match ty {
        syn::Type::Tuple(tuple) => tuple.elems.iter().cloned().collect(),
        other => vec![other.clone()],
    }
}

fn collect_pat_idents(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(ident) => {
            let mut name = ident.ident.to_string();
            if name != "_" {
                if let Some(stripped) = name.strip_prefix('_') {
                    if !stripped.is_empty() {
                        name = stripped.to_string();
                    }
                }
                out.push(name);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_pat_idents(elem, out);
            }
        }
        syn::Pat::TupleStruct(tuple_struct) => {
            for elem in &tuple_struct.elems {
                collect_pat_idents(elem, out);
            }
        }
        _ => {}
    }
}

fn path_extractor_info(sig: &syn::Signature) -> Option<(Vec<String>, Vec<syn::Type>)> {
    for (_, param) in extract_sig_params(sig) {
        let Some(inner_ty) = path_wrapper_inner_type(&param.ty) else {
            continue;
        };
        let mut pat_names = Vec::new();
        collect_pat_idents(&param.pat, &mut pat_names);
        return Some((pat_names, flatten_path_inner_types(&inner_ty)));
    }
    None
}

fn infer_path_param_symbols(path: &str, sig: &syn::Signature) -> Vec<PathParamSymbol> {
    let route_names = extract_route_path_param_names(path);
    let (pat_names, path_types) = path_extractor_info(sig).unwrap_or_default();

    let mut ordered_names = if pat_names.is_empty() {
        route_names.clone()
    } else {
        pat_names
    };

    for name in route_names {
        if !ordered_names.iter().any(|known| known == &name) {
            ordered_names.push(name);
        }
    }

    let fallback_ty: syn::Type = syn::parse_quote! { () };
    let mut symbols = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (index, name) in ordered_names.into_iter().enumerate() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Ok(ident) = syn::parse_str::<syn::Ident>(&name) else {
            continue;
        };
        let ty = path_types
            .get(index)
            .cloned()
            .unwrap_or_else(|| fallback_ty.clone());
        symbols.push(PathParamSymbol { ident, name, ty });
    }

    symbols
}

fn generate_path_param_module(
    path: &str,
    sig: &syn::Signature,
    krate: &TokenStream,
) -> TokenStream {
    let symbols = infer_path_param_symbols(path, sig);
    if symbols.is_empty() {
        return quote! {};
    }

    let consts: Vec<TokenStream> = symbols
        .iter()
        .map(|symbol| {
            let ident = &symbol.ident;
            let name = &symbol.name;
            let ty = &symbol.ty;
            quote! {
                pub const #ident: #krate::PathParam<#ty> = #krate::PathParam::new(#name);
            }
        })
        .collect();

    quote! {
        #[allow(non_snake_case)]
        #[allow(non_upper_case_globals)]
        mod path {
            use super::*;
            #(#consts)*
        }
    }
}

// ── Request pipeline (shared by route / SSE / WS entry functions) ────────

/// Parameters shared by the pipeline emitters.
struct Pipeline<'a> {
    krate: TokenStream,
    def: &'a RoutesImplDef,
    fn_ident: &'a syn::Ident,
    fn_name_str: String,
    /// Module-qualified controller name expression for guard contexts.
    controller_name_q: TokenStream,
    meta_mod: syn::Ident,
    plan: &'a EndpointPlan,
    identity_param: Option<&'a IdentityParam>,
    projection: Projection,
    /// Expression of type `&S` naming the state (`&__state` at top level).
    state: TokenStream,
}

impl<'a> Pipeline<'a> {
    fn new(
        def: &'a RoutesImplDef,
        fn_ident: &'a syn::Ident,
        plan: &'a EndpointPlan,
        identity_param: Option<&'a IdentityParam>,
        projection: Projection,
    ) -> Self {
        let controller_name = &def.controller_name;
        Self {
            krate: r2e_core_path(),
            def,
            fn_ident,
            fn_name_str: fn_ident.to_string(),
            controller_name_q: qualified_controller_name(controller_name),
            meta_mod: format_ident!("__r2e_meta_{}", controller_name),
            plan,
            identity_param,
            projection,
            state: quote! { &__state },
        }
    }

    fn project(&self, rejection: TokenStream) -> TokenStream {
        self.projection.return_stmt(&self.krate, rejection, &self.state)
    }

    /// Split the request, snapshot the peer address.
    fn open(&self) -> TokenStream {
        let krate = &self.krate;
        quote! {
            let (mut __parts, __body) = __req.into_parts();
            let __peer_addr = __parts
                .extensions
                .get::<#krate::http::ConnectInfo<::std::net::SocketAddr>>()
                .map(|__info| __info.0);
        }
    }

    /// Pre-auth guards: controller-level (`"*"` context) first, then the
    /// method's own. They run before ANY extraction — no identity, no body.
    fn pre_auth_guards(&self) -> TokenStream {
        let krate = &self.krate;
        let controller_name_q = &self.controller_name_q;
        let fn_name_str = &self.fn_name_str;
        let ctx = |method_name: TokenStream| {
            quote! {
                #krate::PreAuthGuardContext {
                    method_name: #method_name,
                    controller_name: #controller_name_q,
                    headers: &__parts.headers,
                    uri: &__parts.uri,
                    peer_addr: __peer_addr,
                    path_params: #krate::PathParams::EMPTY,
                }
            }
        };
        let check = |recv: TokenStream, field: &syn::Ident, ctx: TokenStream| {
            let project = self.project(quote! { __rej });
            quote! {
                {
                    let __pre_ctx = #ctx;
                    if let Err(__rej) = #krate::PreAuthGuard::check(&#recv.#field, &__pre_ctx).await {
                        #project
                    }
                }
            }
        };
        let ctrl: Vec<TokenStream> = self
            .plan
            .ctrl_pre_fields()
            .iter()
            .map(|f| check(quote! { __ctrl_deco }, f, ctx(quote! { "*" })))
            .collect();
        let method: Vec<TokenStream> = self
            .plan
            .pre_fields()
            .iter()
            .map(|f| check(quote! { __pre_deco }, f, ctx(quote! { #fn_name_str })))
            .collect();
        quote! { #(#ctrl)* #(#method)* }
    }

    /// Bind the receiver: extract the request-scoped data and build the
    /// façade (`__facade`, with `__ctrl: &Façade`), or borrow the core for
    /// `#[anonymous]` endpoints (`__ctrl: &Core`).
    fn bind_receiver(&self) -> TokenStream {
        let krate = &self.krate;
        let controller_name = &self.def.controller_name;
        if self.plan.anonymous {
            return quote! {
                let __ctrl: &#controller_name = &__core;
            };
        }
        let data_name = request_data_ident_for(controller_name);
        let md = data_marker();
        let meta_mod = &self.meta_mod;
        let project = self.project(quote! { __rej });
        quote! {
            let __data = match <#data_name<#md> as #krate::web::extract::RequestData<__R2eS>>
                ::extract(&mut __parts, &__state).await
            {
                Ok(__d) => __d,
                Err(__rej) => #project
            };
            let __facade = #meta_mod::bind_request(__core, __data);
            let __ctrl = &__facade;
        }
    }

    /// Extract the param-level `#[inject(identity)]` (bean-backed, witness in
    /// the marker generic), binding `__arg_<i>` with the declared type.
    fn identity_param(&self, params: &[(usize, &syn::PatType)]) -> TokenStream {
        let Some(id) = self.identity_param else {
            return quote! {};
        };
        let krate = &self.krate;
        let arg = format_ident!("__arg_{}", id.index);
        let ty = &params
            .iter()
            .find(|(i, _)| *i == id.index)
            .expect("identity parameter index out of range")
            .1
            .ty;
        let marker = identity_marker_for(self.fn_ident);
        let project = self.project(quote! { ::core::convert::Into::into(__e) });
        quote_spanned! { ty.span() =>
            let #arg = match <#ty as #krate::web::extract::FromRequestPartsVia<__R2eS, #marker>>
                ::from_request_parts_via(&mut __parts, &__state).await
            {
                Ok(__v) => __v,
                #[allow(unreachable_code)]
                Err(__e) => #project
            };
        }
    }

    /// Extract one `FromRequestParts` value from `__parts` into `binding`,
    /// converting a foreign rejection through the `ToRejection` probe (typed
    /// `Into<Rejection>` when available, opaque `Response` otherwise).
    fn extract_parts(&self, binding: &syn::Ident, ty: &syn::Type) -> TokenStream {
        let krate = &self.krate;
        let project = self.project(quote! { __rej });
        quote_spanned! { ty.span() =>
            let #binding = match <#ty as #krate::http::extract::FromRequestParts<__R2eS>>
                ::from_request_parts(&mut __parts, &__state).await
            {
                Ok(__v) => __v,
                #[allow(unreachable_code)]
                Err(__e) => {
                    use #krate::web::extract::ToRejectionTyped as _;
                    use #krate::web::extract::ToRejectionOpaque as _;
                    let __rej = (&#krate::web::extract::ToRejection::<
                        <#ty as #krate::http::extract::FromRequestParts<__R2eS>>::Rejection,
                    >::new()).convert(__e);
                    #project
                }
            };
        }
    }

    /// Extract the trailing parameter through `FromRequest` — the one
    /// extractor allowed to consume the body. Consumes `__parts` + `__body`.
    fn extract_body(&self, binding: &syn::Ident, ty: &syn::Type) -> TokenStream {
        let krate = &self.krate;
        let project = self.project(quote! { __rej });
        quote_spanned! { ty.span() =>
            let #binding = match <#ty as #krate::http::extract::FromRequest<__R2eS, _>>
                ::from_request(
                    #krate::http::extract::Request::from_parts(__parts, __body),
                    &__state,
                ).await
            {
                Ok(__v) => __v,
                #[allow(unreachable_code)]
                Err(__e) => {
                    use #krate::web::extract::ToRejectionTyped as _;
                    use #krate::web::extract::ToRejectionOpaque as _;
                    let __rej = (&#krate::web::extract::ToRejection::<
                        <#ty as #krate::http::extract::FromRequest<__R2eS, _>>::Rejection,
                    >::new()).convert(__e);
                    #project
                }
            };
        }
    }

    /// The identity expression of a guard context.
    fn guard_identity_expr(&self) -> TokenStream {
        let meta_mod = &self.meta_mod;
        if let Some(id) = self.identity_param {
            // Case A: param-level identity, already extracted.
            let arg = format_ident!("__arg_{}", id.index);
            if id.is_optional {
                quote! { #arg.as_ref() }
            } else {
                quote! { Some(&#arg) }
            }
        } else if self.plan.anonymous {
            // Case C: #[anonymous] — no identity was extracted. Guards still
            // run (e.g. rate limiting) with `identity: None`, typed to the
            // controller's `IdentityType` so `GuardContext<I>` stays pinned.
            quote! { ::core::option::Option::<&#meta_mod::IdentityType>::None }
        } else {
            // Case B: struct-level identity or no identity, read off the façade.
            quote! { #meta_mod::guard_identity(__ctrl) }
        }
    }

    /// Request-head bindings (`__method`, `__uri`, `__headers`, `__extensions`:
    /// all `&`-references) plus `__path_params`.
    ///
    /// `owned` clones the four values out of `__parts` so the head can outlive
    /// later `&mut __parts` extractions (a `RequestHead` kept for `#[managed]`
    /// acquisition after the body was read); otherwise they borrow `__parts`
    /// directly — the borrows end with the last guard check.
    fn head_bindings(&self, owned: bool) -> TokenStream {
        let krate = &self.krate;
        let raw = format_ident!("__raw_path_params");
        let raw_ty: syn::Type = syn::parse_quote! { #krate::http::extract::RawPathParams };
        let raw_extract = self.extract_parts(&raw, &raw_ty);
        let values = if owned {
            quote! {
                let __method_owned = __parts.method.clone();
                let __uri_owned = __parts.uri.clone();
                let __headers_owned = __parts.headers.clone();
                let __extensions_owned = __parts.extensions.clone();
                let __method = &__method_owned;
                let __uri = &__uri_owned;
                let __headers = &__headers_owned;
                let __extensions = &__extensions_owned;
            }
        } else {
            quote! {
                let __method = &__parts.method;
                let __uri = &__parts.uri;
                let __headers = &__parts.headers;
                let __extensions = &__parts.extensions;
            }
        };
        quote! {
            #raw_extract
            let __path_params = #krate::PathParams::from_raw(&__raw_path_params);
            #values
        }
    }

    /// The `RequestHead` handed to `#[managed]` acquisition. Requires
    /// [`Self::head_bindings`] in scope.
    fn request_head(&self) -> TokenStream {
        let krate = &self.krate;
        quote! {
            let __r2e_head = #krate::RequestHead {
                method: __method,
                uri: __uri,
                headers: __headers,
                extensions: __extensions,
                path_params: __path_params,
                peer_addr: __peer_addr,
            };
        }
    }

    /// Post-auth guards: controller-level (`"*"` context, one stateful-guard
    /// bucket per controller) first, then the method's own. Requires
    /// [`Self::head_bindings`] in scope.
    fn guards(&self) -> TokenStream {
        let krate = &self.krate;
        let controller_name_q = &self.controller_name_q;
        let fn_name_str = &self.fn_name_str;
        let identity = self.guard_identity_expr();
        let ctx = |method_name: TokenStream| {
            quote! {
                #krate::GuardContext {
                    method_name: #method_name,
                    controller_name: #controller_name_q,
                    method: __method,
                    headers: __headers,
                    uri: __uri,
                    extensions: __extensions,
                    peer_addr: __peer_addr,
                    path_params: __path_params,
                    identity: #identity,
                }
            }
        };
        let check = |recv: TokenStream, field: &syn::Ident, ctx: TokenStream| {
            let project = self.project(quote! { __rej });
            quote! {
                {
                    let __guard_ctx = #ctx;
                    if let Err(__rej) = #krate::Guard::check(&#recv.#field, &__guard_ctx).await {
                        #project
                    }
                }
            }
        };
        let ctrl: Vec<TokenStream> = self
            .plan
            .ctrl_guard_fields()
            .iter()
            .map(|f| check(quote! { __ctrl_deco }, f, ctx(quote! { "*" })))
            .collect();
        let method: Vec<TokenStream> = self
            .plan
            .method_guard_fields()
            .iter()
            .map(|f| check(quote! { __deco }, f, ctx(quote! { #fn_name_str })))
            .collect();
        quote! { #(#ctrl)* #(#method)* }
    }

    /// Extract the handler's own parameters (`__arg_<i>`), in declaration
    /// order: every one through `FromRequestParts`, except the last, which
    /// goes through `FromRequest` when `last_consumes_body` (consuming
    /// `__parts` + `__body`). When it does not, the body is left untouched.
    fn params(&self, params: &[(usize, &syn::PatType)], last_consumes_body: bool) -> TokenStream {
        let mut out = Vec::new();
        let n = params.len();
        for (pos, (i, pt)) in params.iter().enumerate() {
            let arg = format_ident!("__arg_{}", i);
            if last_consumes_body && pos + 1 == n {
                out.push(self.extract_body(&arg, &pt.ty));
            } else {
                out.push(self.extract_parts(&arg, &pt.ty));
            }
        }
        quote! { #(#out)* }
    }

    /// Automatic garde validation of the given params (autoref
    /// specialization: types without `garde::Validate` compile to a no-op).
    fn validation(&self, params: &[(usize, &syn::PatType)]) -> TokenStream {
        let krate = &self.krate;
        let calls: Vec<TokenStream> = params
            .iter()
            .map(|(i, pt)| {
                let arg = format_ident!("__arg_{}", i);
                let target = if is_wrapper_type(&pt.ty) {
                    // Json<T>, Query<T>, Path<T>, Form<T> → validate the inner .0
                    quote! { &#arg.0 }
                } else {
                    quote! { &#arg }
                };
                let project = self.project(quote! { *__rej });
                quote! {
                    {
                        use #krate::web::validation::__DoValidate as _;
                        use #krate::web::validation::__SkipValidate as _;
                        if let Err(__rej) = (&#krate::web::validation::__AutoValidator(#target)).__maybe_validate() {
                            #project
                        }
                    }
                }
            })
            .collect();
        quote! { #(#calls)* }
    }

    /// `#[managed]` acquisition. `state` names the `&S` to acquire against —
    /// `&__state` at top level, the `Copy` `__state_ref` inside interceptor
    /// closures. Requires `__r2e_head` in scope.
    fn managed_acquire(&self, managed: &[ManagedParam], state: &TokenStream) -> TokenStream {
        let krate = &self.krate;
        let controller_name_str = self.def.controller_name.to_string();
        let fn_name_str = &self.fn_name_str;
        let stmts: Vec<TokenStream> = managed
            .iter()
            .map(|mp| {
                let arg = format_ident!("__arg_{}", mp.index);
                let ty = &mp.ty;
                let project = self.projection.return_stmt(
                    krate,
                    quote! { ::core::convert::Into::into(__e) },
                    state,
                );
                // `quote_spanned!` so a trait-bound error (`T: ManagedResource<S>`
                // not satisfied) points at the user's own `&mut T` parameter.
                quote_spanned! { ty.span() =>
                    let mut #arg = match #krate::ManagedGuard::<#ty, __R2eS>::acquire(
                        #krate::ManagedContext::new(#state, #controller_name_str, #fn_name_str)
                            .with_request(__r2e_head)
                    ).await {
                        Ok(__r) => __r,
                        Err(__e) => #project
                    };
                }
            })
            .collect();
        quote! { #(#stmts)* }
    }

    /// Run the handler call, convert to a `Response`, finalize every managed
    /// resource (reverse order) and project a finalisation failure.
    fn call_and_finalize(
        &self,
        call: &TokenStream,
        managed: &[ManagedParam],
        state: &TokenStream,
    ) -> TokenStream {
        let krate = &self.krate;
        if managed.is_empty() {
            return quote! { #krate::http::response::IntoResponse::into_response(#call) };
        }
        let controller_name_str = self.def.controller_name.to_string();
        let fn_name_str = &self.fn_name_str;
        let releases: Vec<TokenStream> = managed
            .iter()
            .rev()
            .map(|mp| {
                let arg = format_ident!("__arg_{}", mp.index);
                let ty = &mp.ty;
                quote_spanned! { ty.span() =>
                    if let Err(__e) = #arg.finalize(&__managed_outcome).await {
                        #krate::record_managed_finalize_error(
                            &mut __managed_finalize_error,
                            ::core::convert::Into::into(__e),
                            #controller_name_str,
                            #fn_name_str,
                        );
                    }
                }
            })
            .collect();
        let project = self
            .projection
            .return_stmt(krate, quote! { __rej }, state);
        quote! {
            let __result = #call;
            let __response = #krate::http::response::IntoResponse::into_response(__result);
            let __managed_outcome = #krate::ManagedOutcome::from_status(__response.status());
            let mut __managed_finalize_error: ::core::option::Option<#krate::Rejection> =
                ::core::option::Option::None;
            #(#releases)*
            if let ::core::option::Option::Some(__rej) = __managed_finalize_error {
                #project
            }
            __response
        }
    }
}

/// Generics + where-clause of an entry function: the state, the request-data
/// marker (non-anonymous endpoints), the identity-param marker, and the
/// `#[managed]` bounds.
fn entry_generics(
    def: &RoutesImplDef,
    fn_ident: &syn::Ident,
    anonymous: bool,
    identity_param: Option<&IdentityParam>,
    sig: &syn::Signature,
    managed: &[ManagedParam],
    krate: &TokenStream,
) -> (TokenStream, TokenStream) {
    let state = state_generic();
    let sb = state_bounds(krate);
    let mut generics: Vec<TokenStream> = vec![quote! { #state }];
    let mut bounds: Vec<TokenStream> = vec![quote! { #state: #sb }];
    if !anonymous {
        let md = data_marker();
        let data_name = request_data_ident_for(&def.controller_name);
        generics.push(quote! { #md });
        bounds.push(quote! { #data_name<#md>: #krate::web::extract::RequestData<#state> });
    }
    if let Some(id) = identity_param {
        let marker = identity_marker_for(fn_ident);
        let ty = &extract_sig_params(sig)
            .into_iter()
            .find(|(i, _)| *i == id.index)
            .expect("identity parameter index out of range")
            .1
            .ty;
        generics.push(quote! { #marker });
        bounds.push(quote! { #ty: #krate::web::extract::FromRequestPartsVia<#state, #marker> });
    }
    for mp in managed {
        let ty = crate::util::type_utils::staticize_lifetimes(&mp.ty);
        bounds.push(quote! { #ty: #krate::ManagedResource<#state> });
    }
    (quote! { <#(#generics),*> }, quote! { where #(#bounds,)* })
}

/// Turbofish naming the entry function's marker generics from inside the
/// `Controller` impl (where the same idents are the impl's parameters).
fn entry_turbofish(fn_ident: &syn::Ident, anonymous: bool, has_identity_param: bool) -> TokenStream {
    let state = state_generic();
    let mut args: Vec<TokenStream> = vec![quote! { #state }];
    if !anonymous {
        let md = data_marker();
        args.push(quote! { #md });
    }
    if has_identity_param {
        let marker = identity_marker_for(fn_ident);
        args.push(quote! { #marker });
    }
    quote! { ::<#(#args),*> }
}

// ── HTTP route entry function ────────────────────────────────────────────

/// Generate the entry function of an HTTP route.
///
/// # Design invariant
///
/// When interceptors are present, they **always wrap the raw handler call** —
/// `IntoResponse::into_response()` is applied *after* the outermost
/// interceptor, so type-constrained interceptors like `Cache` (which requires
/// `R: Cacheable`) see the handler's native type.
///
/// **Exception:** with `#[managed]` params AND interceptors, the managed
/// lifecycle (acquire → call → finalize) runs inside the interceptor closure
/// and converts to `Response` there, because a finalisation failure must be
/// projected in place of the handler's response. Type-constrained
/// interceptors therefore don't combine with `#[managed]` params.
fn generate_single_handler(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_ident = &rm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &rm.decorators, true, &rm.path, &rm.fn_item.sig);
    let pipeline = Pipeline::new(
        def,
        fn_ident,
        &plan,
        rm.identity_param.as_ref(),
        Projection::for_signature(&rm.fn_item.sig),
    );

    let all_params = extract_sig_params(&rm.fn_item.sig);
    let managed_indices: std::collections::HashSet<usize> =
        rm.managed_params.iter().map(|mp| mp.index).collect();
    let identity_index = rm.identity_param.as_ref().map(|p| p.index);
    // Request-extracted params: everything but `#[managed]` and the identity.
    let extracted: Vec<(usize, &syn::PatType)> = all_params
        .iter()
        .copied()
        .filter(|(i, _)| !managed_indices.contains(i) && Some(*i) != identity_index)
        .collect();

    let call_args: Vec<TokenStream> = all_params
        .iter()
        .map(|(i, _)| {
            let arg = format_ident!("__arg_{}", i);
            if managed_indices.contains(i) {
                quote! { #arg.resource_mut() }
            } else {
                quote! { #arg }
            }
        })
        .collect();
    let call = if rm.fn_item.sig.asyncness.is_some() {
        quote! { __ctrl.#fn_ident(#(#call_args),*).await }
    } else {
        quote! { __ctrl.#fn_ident(#(#call_args),*) }
    };

    let has_managed = !rm.managed_params.is_empty();
    let has_guards = plan.has_guards();
    let has_body = !extracted.is_empty();
    let needs_head = has_guards || has_managed;
    // The `RequestHead` kept for managed acquisition must survive the body
    // extraction (which consumes `__parts`), so the head values are cloned out
    // first — the same four clones the former axum head extractors made.
    // Guards alone borrow `__parts` and release it before any extractor runs.
    let head_owned = has_managed && has_body;

    let open = pipeline.open();
    let pre_auth = pipeline.pre_auth_guards();
    let bind = pipeline.bind_receiver();
    let identity = pipeline.identity_param(&all_params);
    let head = needs_head.then(|| pipeline.head_bindings(head_owned));
    let request_head = has_managed.then(|| pipeline.request_head());
    let guards = pipeline.guards();
    let params = pipeline.params(&extracted, true);
    let body_sink = (!has_body).then(|| quote! { let _ = __body; });
    let validation = pipeline.validation(&extracted);

    let fn_name_str = fn_ident.to_string();
    let controller_name_str = controller_name.to_string();
    let inner = if plan.has_intercepts() {
        if has_managed {
            // Managed lifecycle inside the interceptor closure (see the
            // invariant above). `__state_ref` is `Copy`, so the nested
            // closures can capture it for acquisition and projection.
            let state_ref = quote! { __state_ref };
            let acquire = pipeline.managed_acquire(&rm.managed_params, &state_ref);
            let finalize = pipeline.call_and_finalize(&call, &rm.managed_params, &state_ref);
            let wrapped = super::decorators::wrap_with_interceptor_refs(
                quote! { #acquire #finalize },
                &fn_name_str,
                &controller_name_str,
                &plan.interceptor_refs(),
                &krate,
            );
            quote! {
                {
                    let __state_ref: &__R2eS = &__state;
                    #wrapped
                }
            }
        } else {
            let wrapped = super::decorators::wrap_with_interceptor_refs(
                call.clone(),
                &fn_name_str,
                &controller_name_str,
                &plan.interceptor_refs(),
                &krate,
            );
            quote! { #krate::http::response::IntoResponse::into_response(#wrapped) }
        }
    } else {
        let state = quote! { &__state };
        let acquire = pipeline.managed_acquire(&rm.managed_params, &state);
        let finalize = pipeline.call_and_finalize(&call, &rm.managed_params, &state);
        quote! { #acquire #finalize }
    };

    let (generics, where_clause) = entry_generics(
        def,
        fn_ident,
        plan.anonymous,
        rm.identity_param.as_ref(),
        &rm.fn_item.sig,
        &rm.managed_params,
        &krate,
    );
    let deco_params = plan.entry_params();
    let deco_items = &plan.deco_items;
    let predeco_items = &plan.predeco_items;
    let invocation = invocation_ident_for(controller_name, fn_ident);
    let core_ty = quote! { ::std::sync::Arc<#controller_name> };

    quote! {
        #deco_items
        #predeco_items

        #[allow(non_snake_case, unused_variables, unused_mut)]
        #[allow(clippy::too_many_arguments)]
        async fn #invocation #generics(
            __state: __R2eS,
            __req: #krate::http::extract::Request,
            #(#deco_params,)*
            __core: #core_ty,
        ) -> #krate::http::response::Response #where_clause {
            #open
            #pre_auth
            #bind
            #identity
            #head
            #request_head
            #guards
            #params
            #body_sink
            #validation
            #inner
        }
    }
}

// ── SSE entry function ───────────────────────────────────────────────────

/// Generate the entry function of an `#[sse("/path")]` method.
///
/// Same pipeline as an HTTP route minus interceptors and `#[managed]`; the
/// stream is wrapped in the shutdown terminator and the keep-alive policy.
fn generate_sse_handler(def: &RoutesImplDef, sm: &SseMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_ident = &sm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &sm.decorators, false, &sm.path, &sm.fn_item.sig);
    let pipeline = Pipeline::new(
        def,
        fn_ident,
        &plan,
        sm.identity_param.as_ref(),
        Projection::Default,
    );

    let all_params = extract_sig_params(&sm.fn_item.sig);
    let identity_index = sm.identity_param.as_ref().map(|p| p.index);
    let extracted: Vec<(usize, &syn::PatType)> = all_params
        .iter()
        .copied()
        .filter(|(i, _)| Some(*i) != identity_index)
        .collect();
    let call_args: Vec<TokenStream> = all_params
        .iter()
        .map(|(i, _)| {
            let arg = format_ident!("__arg_{}", i);
            quote! { #arg }
        })
        .collect();
    let call = if sm.fn_item.sig.asyncness.is_some() {
        quote! { __ctrl.#fn_ident(#(#call_args),*).await }
    } else {
        quote! { __ctrl.#fn_ident(#(#call_args),*) }
    };

    let keep_alive = match sm.keep_alive {
        SseKeepAlive::Default => quote! {
            #krate::http::response::Sse::new(__stream)
                .keep_alive(#krate::http::response::SseKeepAlive::default())
        },
        SseKeepAlive::Interval(secs) => quote! {
            #krate::http::response::Sse::new(__stream)
                .keep_alive(
                    #krate::http::response::SseKeepAlive::new()
                        .interval(::std::time::Duration::from_secs(#secs))
                )
        },
        SseKeepAlive::Disabled => quote! { #krate::http::response::Sse::new(__stream) },
    };

    let has_body = !extracted.is_empty();
    let open = pipeline.open();
    let pre_auth = pipeline.pre_auth_guards();
    let bind = pipeline.bind_receiver();
    let identity = pipeline.identity_param(&all_params);
    let head = plan.has_guards().then(|| pipeline.head_bindings(false));
    let guards = pipeline.guards();
    let params = pipeline.params(&extracted, true);
    let body_sink = (!has_body).then(|| quote! { let _ = __body; });
    let validation = pipeline.validation(&extracted);

    let (generics, where_clause) = entry_generics(
        def,
        fn_ident,
        plan.anonymous,
        sm.identity_param.as_ref(),
        &sm.fn_item.sig,
        &[],
        &krate,
    );
    let deco_params = plan.entry_params();
    let deco_items = &plan.deco_items;
    let predeco_items = &plan.predeco_items;
    let invocation = invocation_ident_for(controller_name, fn_ident);
    let core_ty = quote! { ::std::sync::Arc<#controller_name> };

    quote! {
        #deco_items
        #predeco_items

        #[allow(non_snake_case, unused_variables, unused_mut)]
        #[allow(clippy::too_many_arguments)]
        async fn #invocation #generics(
            __state: __R2eS,
            __req: #krate::http::extract::Request,
            #(#deco_params,)*
            __core: #core_ty,
            __r2e_shutdown: ::core::option::Option<#krate::rt::ShutdownToken>,
        ) -> #krate::http::response::Response #where_clause {
            #open
            #pre_auth
            #bind
            #identity
            #head
            #guards
            #params
            #body_sink
            #validation
            let __stream = #krate::web::sse::until_shutdown(#call, __r2e_shutdown);
            #krate::http::response::IntoResponse::into_response(#keep_alive)
        }
    }
}

// ── WS entry function ────────────────────────────────────────────────────

/// Generate the entry function of a `#[ws("/path")]` method plus its
/// post-upgrade session body.
///
/// The entry function runs the pipeline (pre-auth guards, request data,
/// identity, guards, the non-socket params), extracts the `WebSocketUpgrade`
/// and hands the receiver + params to the upgrade callback, which runs the
/// session body on the tracked lane (`WsSessions::run_session`).
fn generate_ws_handler(def: &RoutesImplDef, wm: &WsMethod) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let fn_ident = &wm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &wm.decorators, false, &wm.path, &wm.fn_item.sig);
    let pipeline = Pipeline::new(
        def,
        fn_ident,
        &plan,
        wm.identity_param.as_ref(),
        Projection::Default,
    );
    let receiver_ty = receiver_ty_for(plan.anonymous, controller_name);
    let fn_name_str = fn_ident.to_string();

    // How a live session names itself in the `shutdown_grace_period` warning.
    // `ws:` + the declaration site, mirroring `spawn_service`'s component type
    // name — the route path is not usable here (it lives on `#[controller]`, as
    // the `PATH_PREFIX` const, and `#[routes]` never sees its value).
    let session_label = syn::LitStr::new(
        &format!("ws:{controller_name}::{fn_name_str}"),
        fn_ident.span(),
    );

    let all_params = extract_sig_params(&wm.fn_item.sig);
    let ws_param_index = wm.ws_param.as_ref().map(|p| p.index);
    let identity_index = wm.identity_param.as_ref().map(|p| p.index);
    // Request-extracted params: everything but the socket and the identity.
    // None of them may consume the body: the upgrade is the trailing
    // extractor, so every handler param goes through `FromRequestParts`.
    let extracted: Vec<(usize, &syn::PatType)> = all_params
        .iter()
        .copied()
        .filter(|(i, _)| Some(*i) != ws_param_index && Some(*i) != identity_index)
        .collect();
    // Forwarded into the session body: every param but the socket.
    let forwarded: Vec<(usize, &syn::PatType)> = all_params
        .iter()
        .copied()
        .filter(|(i, _)| Some(*i) != ws_param_index)
        .collect();
    let session_params: Vec<TokenStream> = forwarded
        .iter()
        .map(|(i, pt)| {
            let arg = format_ident!("__arg_{}", i);
            let ty = &pt.ty;
            quote! { #arg: #ty }
        })
        .collect();
    let forwarded_args: Vec<TokenStream> = forwarded
        .iter()
        .map(|(i, _)| {
            let arg = format_ident!("__arg_{}", i);
            quote! { #arg }
        })
        .collect();

    // The session body: WsStream/WebSocket param, or a returned `WsHandler`.
    let session_body = if let Some(ref ws_p) = wm.ws_param {
        let call_args: Vec<TokenStream> = all_params
            .iter()
            .map(|(i, _)| {
                if Some(*i) == ws_param_index {
                    if ws_p.is_ws_stream {
                        quote! { __ws_stream }
                    } else {
                        quote! { __socket }
                    }
                } else {
                    let arg = format_ident!("__arg_{}", i);
                    quote! { #arg }
                }
            })
            .collect();
        // `with_shutdown`, not `new`: this is what makes the session's receive
        // loop end itself (1001 Going Away) when the app shuts down. A raw
        // `WebSocket` param opts out — it is still tracked, but only its own
        // loop can decide when to stop.
        let setup = ws_p.is_ws_stream.then(|| {
            quote! {
                let __ws_stream = #krate::web::ws::WsStream::with_shutdown(__socket, __shutdown);
            }
        });
        let call = if wm.fn_item.sig.asyncness.is_some() {
            quote! { __ctrl.#fn_ident(#(#call_args),*).await; }
        } else {
            quote! { __ctrl.#fn_ident(#(#call_args),*); }
        };
        quote! { #setup #call }
    } else {
        let call_args: Vec<TokenStream> = all_params
            .iter()
            .map(|(i, _)| {
                let arg = format_ident!("__arg_{}", i);
                quote! { #arg }
            })
            .collect();
        let call = if wm.fn_item.sig.asyncness.is_some() {
            quote! { let __handler = __ctrl.#fn_ident(#(#call_args),*).await; }
        } else {
            quote! { let __handler = __ctrl.#fn_ident(#(#call_args),*); }
        };
        quote! {
            #call
            #krate::web::ws::run_ws_handler(
                #krate::web::ws::WsStream::with_shutdown(__socket, __shutdown),
                __handler,
            ).await;
        }
    };

    let open = pipeline.open();
    let pre_auth = pipeline.pre_auth_guards();
    let bind = pipeline.bind_receiver();
    let identity = pipeline.identity_param(&all_params);
    let head = plan.has_guards().then(|| pipeline.head_bindings(false));
    let guards = pipeline.guards();
    let params = pipeline.params(&extracted, false);
    let validation = pipeline.validation(&extracted);
    let upgrade_ident = format_ident!("__ws_upgrade");
    let upgrade_ty: syn::Type = syn::parse_quote! { #krate::http::ws::WebSocketUpgrade };
    let upgrade = pipeline.extract_parts(&upgrade_ident, &upgrade_ty);

    // The upgrade callback owns the receiver for the whole socket lifetime:
    // the façade (its `Arc` + request data) or the core `Arc` itself.
    let owned_receiver = if plan.anonymous {
        quote! { let __owned = __core; }
    } else {
        quote! { let __owned = __facade; }
    };

    let (generics, where_clause) = entry_generics(
        def,
        fn_ident,
        plan.anonymous,
        wm.identity_param.as_ref(),
        &wm.fn_item.sig,
        &[],
        &krate,
    );
    let deco_params = plan.entry_params();
    let deco_items = &plan.deco_items;
    let predeco_items = &plan.predeco_items;
    let invocation = invocation_ident_for(controller_name, fn_ident);
    let session = ws_session_ident_for(controller_name, fn_ident);
    let core_ty = quote! { ::std::sync::Arc<#controller_name> };

    quote! {
        #deco_items
        #predeco_items

        #[allow(non_snake_case, unused_variables)]
        #[allow(clippy::too_many_arguments)]
        async fn #session(
            __ctrl: &#receiver_ty,
            #(#session_params,)*
            __socket: #krate::http::ws::WebSocket,
            __shutdown: ::core::option::Option<#krate::rt::CancelToken>,
        ) {
            #session_body
        }

        #[allow(non_snake_case, unused_variables, unused_mut)]
        #[allow(clippy::too_many_arguments)]
        async fn #invocation #generics(
            __state: __R2eS,
            __req: #krate::http::extract::Request,
            #(#deco_params,)*
            __core: #core_ty,
            __ws_sessions: #krate::builder::WsSessions,
        ) -> #krate::http::response::Response #where_clause {
            #open
            #pre_auth
            #bind
            #identity
            #head
            #guards
            #params
            #validation
            #upgrade
            let _ = __body;
            #owned_receiver
            #krate::http::response::IntoResponse::into_response(
                __ws_upgrade.on_upgrade(move |__socket| async move {
                    // The session body does NOT run in this detached task when
                    // the app is served through `run()`: `run_session` moves it
                    // to the tracked lane, so shutdown joins it under
                    // `shutdown_grace_period` instead of killing it with the
                    // runtime. Unserved apps (`TestApp`) run it right here.
                    __ws_sessions.run_session(#session_label, move |__shutdown| async move {
                        #session(
                            &__owned,
                            #(#forwarded_args,)*
                            __socket,
                            __shutdown,
                        ).await
                    }).await
                })
            )
        }
    }
}

// ── Registration closures ────────────────────────────────────────────────
//
// Each endpoint registers a `move |State(state), req| async move { entry(..) }`
// closure. The closure captures the controller core `Arc` and the prebuilt
// decorator sets once at registration; axum clones it per request (one `Arc`
// increment each), and the body moves those clones into the entry function.

/// The shared closure shape. `extra_setup` runs at registration time (after
/// the decorator captures), `extra_args` are forwarded after `__core`.
fn registration_closure(
    def: &RoutesImplDef,
    fn_ident: &syn::Ident,
    plan: &EndpointPlan,
    has_identity_param: bool,
    extra_setup: TokenStream,
    extra_args: Vec<TokenStream>,
) -> TokenStream {
    let krate = r2e_core_path();
    let state = state_generic();
    let invocation = invocation_ident_for(&def.controller_name, fn_ident);
    let turbofish = entry_turbofish(fn_ident, plan.anonymous, has_identity_param);
    let capture_setup = plan.capture_setup();
    let capture_args = plan.capture_args();
    quote! {
        {
            let __core_capture = __ctrl.clone();
            #capture_setup
            #extra_setup
            move |#krate::http::extract::State(__state): #krate::http::extract::State<#state>,
                  __req: #krate::http::extract::Request| {
                async move {
                    #invocation #turbofish(
                        __state,
                        __req,
                        #(#capture_args,)*
                        __core_capture,
                        #(#extra_args,)*
                    ).await
                }
            }
        }
    }
}

/// Generate the closure registering an HTTP route.
pub(super) fn generate_route_closure(def: &RoutesImplDef, rm: &RouteMethod) -> TokenStream {
    let fn_ident = &rm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &rm.decorators, true, &rm.path, &rm.fn_item.sig);
    registration_closure(
        def,
        fn_ident,
        &plan,
        rm.identity_param.is_some(),
        quote! {},
        Vec::new(),
    )
}

/// Generate the closure registering an `#[sse]` endpoint. The shutdown token
/// is resolved ONCE here, at registration — not per request. Absent bean (an
/// app with no graph) = `None` = no termination wrapper behaviour.
pub(super) fn generate_sse_closure(def: &RoutesImplDef, sm: &SseMethod) -> TokenStream {
    let krate = r2e_core_path();
    let fn_ident = &sm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &sm.decorators, false, &sm.path, &sm.fn_item.sig);
    registration_closure(
        def,
        fn_ident,
        &plan,
        sm.identity_param.is_some(),
        quote! { let __shutdown_capture = #krate::web::sse::shutdown_token_of(__ctx); },
        vec![quote! { __shutdown_capture }],
    )
}

/// Generate the closure registering a `#[ws]` endpoint. The session registry
/// is a framework bean (not part of the state HList): resolved ONCE here from
/// the bean context and cloned into every upgrade.
pub(super) fn generate_ws_closure(def: &RoutesImplDef, wm: &WsMethod) -> TokenStream {
    let krate = r2e_core_path();
    let fn_ident = &wm.fn_item.sig.ident;
    let plan = plan_endpoint(def, fn_ident, &wm.decorators, false, &wm.path, &wm.fn_item.sig);
    registration_closure(
        def,
        fn_ident,
        &plan,
        wm.identity_param.is_some(),
        quote! { let __ws_sessions_capture = #krate::builder::WsSessions::from_context(__ctx); },
        vec![quote! { __ws_sessions_capture }],
    )
}
