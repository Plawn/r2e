# Feature 20 — Proxy & Catch-All Routes

## Objective

Make proxy-shaped traffic (registry proxies, API gateways, protocol multiplexers) expressible as **first-class controller routes** — with DI, guards, interceptors, config, and TestApp support — instead of a hand-written axum fallback mounted through `with_layer_fn` with a closure-captured state clone.

Two route kinds plus raw-request access:

- **`#[any("/path")]`** — matches every HTTP method (`axum::routing::any`); combine with a `{*wildcard}` path segment for prefix-scoped proxying.
- **`#[fallback]`** — controller-scoped catch-all registered as the app-wide `Router::fallback`: handles every request no other route matched, for any method.
- **Raw `Request` parameter** — any route handler can take `r2e::http::extract::Request` as its **last** parameter for full access to method, URI, headers, and the streaming body.

## `#[any]` — all-methods wildcard routes

```rust
use r2e::http::extract::Request;
use r2e::http::response::Response;
use r2e::http::Body;
use r2e::prelude::*;

#[controller]
pub struct ProxyController {
    #[inject]
    upstream: UpstreamClient,
}

#[routes]
impl ProxyController {
    #[any("/registry/{*path}")]
    async fn proxy(&self, req: Request) -> Response {
        // Route on req.method() / req.uri() / req.headers(),
        // stream req.into_body() wherever it needs to go.
        self.upstream.forward(req).await
    }
}
```

- The wildcard segment requires at least one path segment: `/registry/{*path}` matches `/registry/a/b` but not `/registry` or `/registry/`.
- Guards, interceptors, `#[inject]`/`#[config]`, identity params, `#[middleware]`/`#[layer]` all work exactly as on verb routes.
- `#[roles]` works too, but per-protocol auth (e.g. Docker's `WWW-Authenticate` challenge dance) usually belongs in the handler body.

## `#[fallback]` — the app-wide catch-all

```rust
#[routes]
impl ProxyController {
    #[fallback]
    async fn dispatch(&self, req: Request) -> Response {
        // Content-type / path-prefix protocol dispatch, custom 404s, ...
    }
}
```

Rules (all compile errors when violated):

- takes **no path argument** — it matches whatever no other route matched;
- at most **one** `#[fallback]` per controller;
- only on controllers **without a path prefix** (`#[controller]` with no `path`, or `path = "/"`) — the fallback is app-wide, a prefix would be misleading;
- cannot be combined with a verb attribute (`#[get]`, `#[any]`, ...);
- `#[pre_guard]`, `#[middleware]`, and `#[layer]` are not supported (they attach to a `.route(...)` registration; `Router::fallback` takes a bare handler) — `#[guard]` and `#[intercept]` run inside the handler and work as usual.

Runtime constraints:

- If **two registered controllers** both declare a `#[fallback]`, the router build panics (axum allows a single fallback per app: `Cannot merge two Routers that both have a fallback`).
- **Declared routes always win.** A path that matches with the wrong method returns **405**, not the fallback (correct HTTP semantics).
- **NormalizePath composes**: with the `NormalizePath` plugin enabled, the trailing slash is stripped by a pre-routing URI rewrite; the trimmed request is routed once, and if it matches nothing it lands in the controller fallback.

## Raw `Request` and streaming responses

Handler parameters are passed to axum verbatim, so `Request` (which consumes the body) must be the **last** parameter. Responses stream through plain `IntoResponse` values:

```rust
// Pass-through: reuse the incoming body as the response body (zero buffering)
Response::builder().status(200).body(Body::new(req.into_body())).unwrap()

// Generated stream
Body::from_stream(some_try_stream_of_bytes)

// Redirect to a presigned URL
Redirect::temporary(&url).into_response()
```

## OpenAPI

`#[any]` routes, `#[fallback]` routes, and any route with a `{*wildcard}` path are **excluded from the OpenAPI spec**: they have no single documentable method/shape, and `{*path}` is not a valid OpenAPI path template.

## The escape-hatch ladder

Reach for the first rung that fits; each step down trades framework integration for control:

1. **`#[any]` / `#[fallback]` controller routes** — full DI, guards, interceptors, TestApp. This is the right level for proxies and gateways.
2. **`merge_router(router)` / `register_routes(router)`** — merge a raw `Router<T>` fragment before state application: raw routes share the app state and global plugins (CORS, tracing, error handling) but get no controller DI, guards, or interceptors.
3. **`with_layer_fn(|router| ...)`** — transform the final state-erased `Router` (add tower layers, nest sub-apps). Runs **after** `with_state`, so the `State` extractor is unusable inside — capture what you need in the closure.

## Testing

Everything works through `TestApp` / `oneshot` as usual:

```rust
#[r2e::test(app = my_app::app)]
async fn fallback_catches_unmatched(app: TestApp) {
    let resp = app.get("/definitely/not/a/route").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
```

See `r2e-core/tests/proxy_routes.rs` (machinery), `examples/example-app/src/controllers/proxy_controller.rs` + `tests/proxy_test.rs` (blueprint usage), and `r2e-compile-tests/compile-fail/fallback_*.rs` (diagnostics).
