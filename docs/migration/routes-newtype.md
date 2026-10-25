# Migration: `Routes` replaces `axum::Router` (r2e-core 0.7 → 0.8)

The HTTP surface R2E hands you — `AppBuilder::build()`, `Controller::routes`,
plugin hooks, `TestApp::new`, `merge_router`, `with_layer_fn` — used to be
axum's `Router<S>` re-exported as `r2e::http::Router`. It is now **R2E's own
type, `r2e::http::Routes<S>`**, a newtype over the axum router with the same
method vocabulary (`route`, `nest`, `merge`, `layer`, `route_layer`,
`fallback`, `with_state`, …) and R2E-owned bounds. Likewise `MethodRouter`
became `MethodRoutes<S>` and the `get`/`post`/… builders under
`r2e::http::routing` return it. The point is containment (decision D3 of
`plans/axum-containment-roadmap.md`): nothing outside `r2e-http` names an axum
routing type any more, so the backend can change without a second migration.

Everything that goes through controllers (`#[controller]` + `#[routes]`),
guards, interceptors, `#[layer(..)]`, plugins that only call `after_routes` /
`add_layer` with closures, and `TestApp::boot` rebuilds unchanged. The breaks
are on code that **spelled** the axum types or wrote raw axum handlers.

## What breaks

| Before (0.7) | After (0.8) |
|---|---|
| `use r2e::http::Router;` / `r2e::prelude::Router` | `use r2e::http::Routes;` / `r2e::prelude::Routes` (same generic `S`) |
| `r2e::http::routing::MethodRouter` | `r2e::http::routing::MethodRoutes` |
| `Router::new().route("/x", get(\|\| async { "hi" }))` | `Routes::new().route("/x", get(\|_, _\| async { "hi" }))` — handlers are `FnOnce(S, Request) -> Fut`; run extractors yourself (`Query::<Q>::from_request_parts(&mut parts, &())`) |
| `router.layer(middleware::from_fn(f))` | `routes.middleware_fn(f)` (`f: async fn(Request, Next) -> Response`) |
| `router.layer(middleware::from_fn_with_state(st, f))` | `routes.middleware_fn_with_state(st, f)` (`f: async fn(St, Request, Next) -> Response`) |
| `#[middleware(my_fn, state)]` with `State(ctrl): State<Arc<Self>>` | `my_fn(ctrl: Arc<Self>, req: Request, next: Next)` — the core is a plain leading argument |
| `BootableApp::into_router()` / `into_router_with_consumers()` | `into_routes()` / `into_routes_with_consumers()` |
| `PreparedApp::router()` / `router_mut()`, `RunningApp::router()` | `routes()` / `routes_mut()`, `routes()` |
| `EmbeddedFrontend::…build().into_router()` (`r2e-static`) | `.into_routes()` |
| `r2e::http::serve` / `ListenerExt` (axum's server entry points) | `r2e::http::server::serve(listener, routes, shutdown, ServeOptions)` (phase A) |
| `r2e::http::middleware::{from_fn, from_fn_with_state}`, `r2e::http::routing::Route` as axum's | removed from `r2e::http`; `Route` is now R2E's own newtype implementing `tower::Service` |

Names that stay: `merge_router`, `register_routes`, `with_layer_fn`,
`add_layer`, `wrap_router`, `RouterWrap` — only their parameter type changed
from `Router` to `Routes`. `has_custom_fallback` is now a method
(`Routes::has_custom_fallback(&self)`).

Each break is a compile error naming the missing item (`cannot find type
'Router' in module 'r2e::http'`, `no function or associated item named
'into_router'`).

## Raw handlers on `Routes`

A `Routes::route` / `MethodRoutes` handler is a closure or fn taking the state
and the request, returning a future whose output is `IntoResponse`. It is the
shape the controller macros already generate; for a hand-written route:

```rust,ignore
use r2e::http::extract::{FromRequestParts, Query, Request};
use r2e::http::routing::get;
use r2e::http::Routes;

async fn search(_: (), req: Request) -> Result<String, QueryRejection> {
    let (mut parts, _) = req.into_parts();
    let Query(q) = Query::<SearchQuery>::from_request_parts(&mut parts, &()).await?;
    Ok(q.term)
}

let routes: Routes = Routes::new().route("/search", get(search));
```

A stateful route receives the app state as the first argument
(`get(|state: AppState, _req| async move { state.greeting })`). Typed bodies:
`<Json<T> as FromRequest<()>>::from_request(req, &()).await` (the turbofish
disambiguates from `OptionalFromRequest`).

## Function middleware

```rust,ignore
async fn request_id(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert("x-request-id", HeaderValue::from_static("…"));
    res
}

// app-wide
ctx.add_layer(|routes| routes.middleware_fn(request_id));
// on a controller route — unchanged attribute, new `state` signature
#[middleware(tag_response, state)]
async fn tag_response(ctrl: Arc<ApiController>, req: Request, next: Next) -> Response { /* … */ }
```

## Tower layers

`Routes::layer` / `MethodRoutes::layer` take any `RouteLayer`: a `tower::Layer`
whose service is infallible and `IntoResponse`-producing — every tower-http
layer under `r2e::http::layers` and the usual `ServiceBuilder` stacks qualify.
A layer written against `axum::middleware::Next` or
`axum::extract::State` must move to `Next`/plain arguments as above, or use the
escape hatch.

## Escape hatch

`r2e::http::axum_compat::{into_router, from_router, into_method_router,
from_method_router}` convert both ways for the rare third-party crate that
takes an `axum::Router`. It is outside R2E's public promise and deliberately
greppable (`scripts/check-source-boundary.sh`, group `axum-shape`).
