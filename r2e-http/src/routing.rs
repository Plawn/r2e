//! R2E's routing surface: [`Routes`], [`MethodRoutes`] and the handler shape
//! they take.
//!
//! These are R2E-owned newtypes over the backend's router. They exist so that
//! nothing above `r2e-http` names the backend's routing types
//! (`plans/axum-containment-roadmap.md`, phase C / decision D3): the
//! controller codegen emits `Routes::route(path, get(closure))`, plugins hand
//! back `Routes`, `AppBuilder::build()` returns `Routes`, and the one serve
//! path ([`crate::server::serve`]) consumes it. A backend swap rewrites this
//! module and the generated code keeps compiling.
//!
//! # The handler shape
//!
//! A route handler is R2E's shape, not the backend's extractor tuple: an
//! async function of the **state** and the **request**,
//! `async fn(S, Request) -> impl IntoResponse` (see [`Handler`]). That is
//! exactly what the controller codegen already produces — the generated entry
//! fn does its own extraction through `FromRequestPartsVia` — so no
//! backend extractor machinery leaks through this surface. A hand-written
//! handler that needs an extractor calls it directly on the request:
//!
//! ```rust
//! use r2e_http::extract::{FromRequestParts as _, Query};
//! use r2e_http::response::IntoResponse;
//! use r2e_http::routing::{get, Routes};
//! use r2e_http::{Request, Response};
//!
//! async fn echo(_state: (), req: Request) -> Response {
//!     let (mut parts, _body) = req.into_parts();
//!     match Query::<Vec<(String, String)>>::from_request_parts(&mut parts, &()).await {
//!         Ok(Query(pairs)) => format!("{pairs:?}").into_response(),
//!         Err(rejection) => rejection.into_response(),
//!     }
//! }
//!
//! let routes: Routes = Routes::new().route("/echo", get(echo));
//! # let _ = routes;
//! ```
//!
//! # Layers
//!
//! [`Routes::layer`] / [`MethodRoutes::layer`] take any `tower::Layer` whose
//! service speaks `Request` → `Response` infallibly; the bound is spelled over
//! R2E's [`Route`] (the type a layer wraps) so a layer written against R2E
//! never names a backend type. Function middleware goes through
//! [`Routes::middleware_fn`] / [`middleware_fn_with_state`](Routes::middleware_fn_with_state)
//! with R2E's [`Next`].
//!
//! The backend router behind a `Routes` is reachable, on purpose and
//! greppably, through [`crate::axum_compat::into_router`] /
//! [`crate::axum_compat::from_router`].

use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::task::{Context, Poll};

use axum::extract::State;
use axum::routing::{MethodFilter, MethodRouter};
use tower::{Layer, Service};

use crate::middleware::Next;
use crate::{IntoResponse, Request};

// ── Handler ────────────────────────────────────────────────────────────────

/// A route handler: an async function of the route state and the request.
///
/// Implemented for every `FnOnce(S, Request) -> Fut` that is `Clone + Send +
/// Sync + 'static` and whose future is `Send` with an [`IntoResponse`]
/// output — i.e. plain `async fn handler(state: S, req: Request) -> R` and
/// `move |state, req| async move { .. }` closures. There is nothing else to
/// implement: this is the shape the controller codegen emits, and the only
/// one the routing surface accepts.
pub trait Handler<S>: Clone + Send + Sync + 'static {
    /// What the handler resolves to; converted with [`IntoResponse`].
    type Output: IntoResponse;
    /// The handler's future.
    type Future: Future<Output = Self::Output> + Send;

    /// Run the handler for one request.
    fn call(self, state: S, req: Request) -> Self::Future;
}

impl<F, S, Fut> Handler<S> for F
where
    F: FnOnce(S, Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future + Send,
    Fut::Output: IntoResponse,
{
    type Output = Fut::Output;
    type Future = Fut;

    #[inline]
    fn call(self, state: S, req: Request) -> Fut {
        (self)(state, req)
    }
}

/// Bridge an R2E [`Handler`] to the backend's handler contract.
///
/// The backend wants an extractor tuple; R2E's shape is `(state, request)`,
/// which is the backend's `(State<S>, Request)` with the wrapper removed. One
/// closure, no per-request cost beyond the clone the backend already does.
fn adapt<S, H>(handler: H) -> impl FnOnce(State<S>, Request) -> H::Future + Clone + Send + Sync + 'static
where
    S: Clone + Send + Sync + 'static,
    H: Handler<S>,
{
    move |State(state): State<S>, req: Request| handler.call(state, req)
}

/// Bridge an R2E function middleware (`Fn(Request, Next)`) to the backend's.
fn adapt_middleware<F, Fut, R>(
    f: F,
) -> impl Fn(Request, axum::middleware::Next) -> Fut + Clone + Send + Sync + 'static
where
    F: Fn(Request, Next) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    move |req: Request, next: axum::middleware::Next| f(req, Next::new(next))
}

/// Bridge an R2E stateful function middleware (`Fn(St, Request, Next)`) to the backend's.
fn adapt_middleware_with_state<St, F, Fut, R>(
    f: F,
) -> impl Fn(State<St>, Request, axum::middleware::Next) -> Fut + Clone + Send + Sync + 'static
where
    St: Clone + Send + Sync + 'static,
    F: Fn(St, Request, Next) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    move |State(state): State<St>, req: Request, next: axum::middleware::Next| {
        f(state, req, Next::new(next))
    }
}

// ── Route (the service a layer wraps) ──────────────────────────────────────

/// The service a route layer wraps: one fully-resolved route or fallback.
///
/// Appears only in the bounds of [`Routes::layer`] / [`MethodRoutes::layer`]
/// (`L: Layer<Route>`). A layer that is generic over its inner service — every
/// tower / tower-http layer, and every layer R2E ships — satisfies that bound
/// automatically; `Route` itself is a `tower::Service<Request>` that forwards
/// to the wrapped route.
#[derive(Clone)]
pub struct Route {
    inner: axum::routing::Route,
}

impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route").finish_non_exhaustive()
    }
}

impl<B> Service<http::Request<B>> for Route
where
    axum::routing::Route: Service<http::Request<B>>,
{
    type Response = <axum::routing::Route as Service<http::Request<B>>>::Response;
    type Error = <axum::routing::Route as Service<http::Request<B>>>::Error;
    type Future = <axum::routing::Route as Service<http::Request<B>>>::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    #[inline]
    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}

/// Presents an R2E-bounded layer (`Layer<Route>`) to the backend as a layer
/// over its own route type. Zero-cost: the wrapped service is the layer's own.
#[derive(Clone)]
struct Adapt<L>(L);

impl<L> Layer<axum::routing::Route> for Adapt<L>
where
    L: Layer<Route>,
{
    type Service = L::Service;

    fn layer(&self, inner: axum::routing::Route) -> Self::Service {
        self.0.layer(Route { inner })
    }
}

/// What a layer's service must be for [`Routes::layer`] and
/// [`MethodRoutes::layer`]: `Request` in, something [`IntoResponse`] out,
/// infallibly, on a `Send` future. Blanket-implemented; never implemented by
/// hand.
pub trait RouteService:
    Service<
        Request,
        Response: IntoResponse + 'static,
        Error: Into<Infallible> + 'static,
        Future: Send + 'static,
    > + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> RouteService for T
where
    T: Service<Request> + Clone + Send + Sync + 'static,
    T::Response: IntoResponse + 'static,
    T::Error: Into<Infallible> + 'static,
    T::Future: Send + 'static,
{
}

/// A layer [`Routes::layer`] / [`MethodRoutes::layer`] accept: a
/// `tower::Layer` over [`Route`] whose service is a [`RouteService`].
/// Blanket-implemented; never implemented by hand.
pub trait RouteLayer: Layer<Route, Service: RouteService> + Clone + Send + Sync + 'static {}

impl<L> RouteLayer for L
where
    L: Layer<Route> + Clone + Send + Sync + 'static,
    L::Service: RouteService,
{
}

/// A leaf service a route or fallback can be served by
/// ([`Routes::route_service`], [`Routes::fallback_service`]).
/// Blanket-implemented; never implemented by hand.
pub trait LeafService:
    Service<Request, Error = Infallible, Response: IntoResponse, Future: Send + 'static>
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> LeafService for T
where
    T: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
    T::Response: IntoResponse,
    T::Future: Send + 'static,
{
}

// ── Methods ────────────────────────────────────────────────────────────────

/// A set of HTTP methods for [`on`] / [`MethodRoutes::on`].
///
/// Combine with `|`: `Methods::GET | Methods::HEAD`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Methods(MethodFilter);

impl Methods {
    /// `CONNECT`.
    pub const CONNECT: Self = Self(MethodFilter::CONNECT);
    /// `DELETE`.
    pub const DELETE: Self = Self(MethodFilter::DELETE);
    /// `GET`.
    pub const GET: Self = Self(MethodFilter::GET);
    /// `HEAD`.
    pub const HEAD: Self = Self(MethodFilter::HEAD);
    /// `OPTIONS`.
    pub const OPTIONS: Self = Self(MethodFilter::OPTIONS);
    /// `PATCH`.
    pub const PATCH: Self = Self(MethodFilter::PATCH);
    /// `POST`.
    pub const POST: Self = Self(MethodFilter::POST);
    /// `PUT`.
    pub const PUT: Self = Self(MethodFilter::PUT);
    /// `TRACE`.
    pub const TRACE: Self = Self(MethodFilter::TRACE);
}

impl std::ops::BitOr for Methods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0.or(rhs.0))
    }
}

// ── MethodRoutes ───────────────────────────────────────────────────────────

/// The handlers for one path, keyed by HTTP method, plus that path's own
/// layers — what [`Routes::route`] takes.
///
/// Build one with the free functions ([`get`], [`post`], …, [`on`], [`any`])
/// and chain further methods on it: `get(list).post(create)`.
pub struct MethodRoutes<S = ()> {
    inner: MethodRouter<S, Infallible>,
}

impl<S> Clone for MethodRoutes<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S> fmt::Debug for MethodRoutes<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<S> Default for MethodRoutes<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! chained_method {
    ($(#[$doc:meta])* $name:ident => $filter:ident) => {
        $(#[$doc])*
        pub fn $name<H>(self, handler: H) -> Self
        where
            H: Handler<S>,
        {
            self.on(Methods::$filter, handler)
        }
    };
}

macro_rules! free_method {
    ($(#[$doc:meta])* $name:ident => $filter:ident) => {
        $(#[$doc])*
        pub fn $name<S, H>(handler: H) -> MethodRoutes<S>
        where
            S: Clone + Send + Sync + 'static,
            H: Handler<S>,
        {
            MethodRoutes::new().on(Methods::$filter, handler)
        }
    };
}

impl<S> MethodRoutes<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// No handler for any method: every request gets the method-not-allowed
    /// fallback until a method is added.
    pub fn new() -> Self {
        Self {
            inner: MethodRouter::new(),
        }
    }

    /// Serve `handler` for every method in `methods`.
    pub fn on<H>(self, methods: Methods, handler: H) -> Self
    where
        H: Handler<S>,
    {
        Self {
            inner: self.inner.on(methods.0, adapt(handler)),
        }
    }

    chained_method!(
        /// Serve `handler` for `GET`.
        get => GET
    );
    chained_method!(
        /// Serve `handler` for `POST`.
        post => POST
    );
    chained_method!(
        /// Serve `handler` for `PUT`.
        put => PUT
    );
    chained_method!(
        /// Serve `handler` for `PATCH`.
        patch => PATCH
    );
    chained_method!(
        /// Serve `handler` for `DELETE`.
        delete => DELETE
    );
    chained_method!(
        /// Serve `handler` for `HEAD`.
        head => HEAD
    );
    chained_method!(
        /// Serve `handler` for `OPTIONS`.
        options => OPTIONS
    );
    chained_method!(
        /// Serve `handler` for `TRACE`.
        trace => TRACE
    );

    /// Serve `handler` for every method no other handler on this path
    /// claims (replaces the method-not-allowed response).
    pub fn fallback<H>(self, handler: H) -> Self
    where
        H: Handler<S>,
    {
        Self {
            inner: self.inner.fallback(adapt(handler)),
        }
    }

    /// Wrap every handler on this path (and its fallback) in `layer`.
    pub fn layer<L>(self, layer: L) -> Self
    where
        L: RouteLayer,
    {
        Self {
            inner: self.inner.layer(Adapt(layer)),
        }
    }

    /// Wrap every handler on this path in the function middleware `f`
    /// (`async fn(Request, Next) -> impl IntoResponse`).
    pub fn middleware_fn<F, Fut, R>(self, f: F) -> Self
    where
        F: Fn(Request, Next) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + 'static,
    {
        Self {
            inner: self
                .inner
                .layer(axum::middleware::from_fn(adapt_middleware(f))),
        }
    }

    /// Like [`middleware_fn`](Self::middleware_fn), with `state` handed to
    /// `f` as its first argument on every request
    /// (`async fn(St, Request, Next) -> impl IntoResponse`).
    pub fn middleware_fn_with_state<St, F, Fut, R>(self, state: St, f: F) -> Self
    where
        St: Clone + Send + Sync + 'static,
        F: Fn(St, Request, Next) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + 'static,
    {
        Self {
            inner: self.inner.layer(axum::middleware::from_fn_with_state(
                state,
                adapt_middleware_with_state(f),
            )),
        }
    }

    /// Provide the state, turning this into a `MethodRoutes<S2>` (usually
    /// `MethodRoutes<()>`) that can be mounted on a stateless `Routes`.
    pub fn with_state<S2>(self, state: S) -> MethodRoutes<S2> {
        MethodRoutes {
            inner: self.inner.with_state(state),
        }
    }

    pub(crate) fn into_inner(self) -> MethodRouter<S, Infallible> {
        self.inner
    }

    pub(crate) fn from_inner(inner: MethodRouter<S, Infallible>) -> Self {
        Self { inner }
    }
}

/// Serve `handler` for every method in `methods`.
pub fn on<S, H>(methods: Methods, handler: H) -> MethodRoutes<S>
where
    S: Clone + Send + Sync + 'static,
    H: Handler<S>,
{
    MethodRoutes::new().on(methods, handler)
}

free_method!(
    /// Serve `handler` for `GET`.
    get => GET
);
free_method!(
    /// Serve `handler` for `POST`.
    post => POST
);
free_method!(
    /// Serve `handler` for `PUT`.
    put => PUT
);
free_method!(
    /// Serve `handler` for `PATCH`.
    patch => PATCH
);
free_method!(
    /// Serve `handler` for `DELETE`.
    delete => DELETE
);
free_method!(
    /// Serve `handler` for `HEAD`.
    head => HEAD
);
free_method!(
    /// Serve `handler` for `OPTIONS`.
    options => OPTIONS
);
free_method!(
    /// Serve `handler` for `TRACE`.
    trace => TRACE
);

/// Serve `handler` for every method.
pub fn any<S, H>(handler: H) -> MethodRoutes<S>
where
    S: Clone + Send + Sync + 'static,
    H: Handler<S>,
{
    MethodRoutes::from_inner(axum::routing::any(adapt(handler)))
}

// ── Routes ─────────────────────────────────────────────────────────────────

/// A set of routes: paths → [`MethodRoutes`], nested and merged sub-trees,
/// a fallback, and the layers around them.
///
/// `S` is the state the handlers receive; [`with_state`](Self::with_state)
/// turns a `Routes<S>` into the stateless `Routes<()>` that is served (and
/// that is a `tower::Service<Request>`, so tests can `oneshot` it).
pub struct Routes<S = ()> {
    inner: axum::Router<S>,
}

impl<S> Clone for Routes<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S> fmt::Debug for Routes<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<S> Default for Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// No routes; unmatched requests get the default (empty-bodied) 404
    /// until a [`fallback`](Self::fallback) is installed.
    pub fn new() -> Self {
        Self {
            inner: axum::Router::new(),
        }
    }

    /// Mount `method_routes` at `path` (`/users/{id}`, `/files/{*rest}`).
    ///
    /// Panics on an invalid path or when `path` is already routed — the same
    /// contract as the backend's.
    pub fn route(self, path: &str, method_routes: MethodRoutes<S>) -> Self {
        Self {
            inner: self.inner.route(path, method_routes.inner),
        }
    }

    /// Mount an arbitrary `tower::Service` at `path`, for every method.
    pub fn route_service<T>(self, path: &str, service: T) -> Self
    where
        T: LeafService,
    {
        Self {
            inner: self.inner.route_service(path, service),
        }
    }

    /// Mount `routes` under the prefix `path`.
    pub fn nest(self, path: &str, routes: Routes<S>) -> Self {
        Self {
            inner: self.inner.nest(path, routes.inner),
        }
    }

    /// Add every route of `other`. Panics on an overlapping path, or when both
    /// sides carry a custom fallback — the same contract as the backend's.
    pub fn merge(self, other: Routes<S>) -> Self {
        Self {
            inner: self.inner.merge(other.inner),
        }
    }

    /// Wrap every route **and the fallback** in `layer`. Runs after routing:
    /// it cannot change which route matches.
    pub fn layer<L>(self, layer: L) -> Self
    where
        L: RouteLayer,
    {
        Self {
            inner: self.inner.layer(Adapt(layer)),
        }
    }

    /// Wrap only the routes added so far (not the fallback) in `layer`.
    pub fn route_layer<L>(self, layer: L) -> Self
    where
        L: RouteLayer,
    {
        Self {
            inner: self.inner.route_layer(Adapt(layer)),
        }
    }

    /// Wrap every route and the fallback in the function middleware `f`
    /// (`async fn(Request, Next) -> impl IntoResponse`).
    pub fn middleware_fn<F, Fut, R>(self, f: F) -> Self
    where
        F: Fn(Request, Next) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + 'static,
    {
        Self {
            inner: self
                .inner
                .layer(axum::middleware::from_fn(adapt_middleware(f))),
        }
    }

    /// Like [`middleware_fn`](Self::middleware_fn), with `state` handed to
    /// `f` as its first argument on every request
    /// (`async fn(St, Request, Next) -> impl IntoResponse`).
    pub fn middleware_fn_with_state<St, F, Fut, R>(self, state: St, f: F) -> Self
    where
        St: Clone + Send + Sync + 'static,
        F: Fn(St, Request, Next) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + 'static,
    {
        Self {
            inner: self.inner.layer(axum::middleware::from_fn_with_state(
                state,
                adapt_middleware_with_state(f),
            )),
        }
    }

    /// Serve `handler` for every request no route matches.
    pub fn fallback<H>(self, handler: H) -> Self
    where
        H: Handler<S>,
    {
        Self {
            inner: self.inner.fallback(adapt(handler)),
        }
    }

    /// Serve `service` for every request no route matches.
    pub fn fallback_service<T>(self, service: T) -> Self
    where
        T: LeafService,
    {
        Self {
            inner: self.inner.fallback_service(service),
        }
    }

    /// Serve `handler` when a path matches but none of its methods does, for
    /// every route still on the default 405 (a route's own
    /// [`MethodRoutes::fallback`] is kept).
    pub fn method_not_allowed_fallback<H>(self, handler: H) -> Self
    where
        H: Handler<S>,
    {
        Self {
            inner: self.inner.method_not_allowed_fallback(adapt(handler)),
        }
    }

    /// Provide the state, turning this into a `Routes<S2>` (usually
    /// `Routes<()>`, the servable form).
    pub fn with_state<S2>(self, state: S) -> Routes<S2> {
        Routes {
            inner: self.inner.with_state(state),
        }
    }
}

impl<S> Routes<S> {
    /// Whether these routes carry a user-installed fallback —
    /// [`fallback`](Self::fallback) / [`fallback_service`](Self::fallback_service)
    /// on them, or merged in from routes that had one — as opposed to the
    /// built-in empty 404. `r2e-core` reads it to install the framework 404
    /// without stepping on a fallback the app (or a plugin) installed.
    ///
    /// The backend keeps this bit privately (it is what makes `merge` panic
    /// on two custom fallbacks) and exposes it only through its `Debug`
    /// output, as the `default_fallback` field. The field is printed after the
    /// two path routers and before `catch_all_fallback` (whose own `Debug`
    /// carries no user data), so the **last** occurrence is always the
    /// top-level one: a route path that happened to spell the field name could
    /// only appear earlier. Pinned to the backend version this crate owns;
    /// `tests/routing.rs` covers both states across `with_state`, `layer`,
    /// `merge` and `nest`.
    #[must_use]
    pub fn has_custom_fallback(&self) -> bool {
        const FIELD: &str = ", default_fallback: ";
        let repr = format!("{:?}", self.inner);
        match repr.rfind(FIELD) {
            Some(at) => repr[at + FIELD.len()..].starts_with("false"),
            None => false,
        }
    }

    pub(crate) fn into_inner(self) -> axum::Router<S> {
        self.inner
    }

    pub(crate) fn from_inner(inner: axum::Router<S>) -> Self {
        Self { inner }
    }
}

impl<B> Service<http::Request<B>> for Routes<()>
where
    axum::Router<()>: Service<http::Request<B>>,
{
    type Response = <axum::Router<()> as Service<http::Request<B>>>::Response;
    type Error = <axum::Router<()> as Service<http::Request<B>>>::Error;
    type Future = <axum::Router<()> as Service<http::Request<B>>>::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    #[inline]
    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}
