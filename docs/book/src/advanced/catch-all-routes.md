# Catch-All and Proxy Routes

Proxies, gateways and protocol dispatchers need routes that accept any method
or any path. R2E offers two attributes for this, `#[any]` and `#[fallback]`.
Both are normal controller routes, so DI, guards, interceptors and `TestApp`
work as usual.

## Setup

No Cargo feature is needed: both attributes are part of the core macros.

## `#[any]`: all methods on one path

```rust
use r2e::http::extract::Request;
use r2e::http::response::Response;
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
        // Route on req.method(), req.uri(), req.headers(),
        // and stream req.into_body() wherever it needs to go.
        self.upstream.forward(req).await
    }
}
```

`{*path}` is a wildcard segment that captures the rest of the path.

## `#[fallback]`: the app-wide catch-all

A `#[fallback]` handler receives every request that no other route matched:

```rust
#[routes]
impl ProxyController {
    #[fallback]
    async fn dispatch(&self, req: Request) -> Response {
        // Protocol dispatch, custom 404 pages, ...
    }
}
```

These rules are checked at compile time:

- `#[fallback]` takes no path.
- At most one `#[fallback]` per controller.
- Only on controllers without a path prefix (`#[controller]` or `path = "/"`).
- It cannot be combined with a verb attribute (`#[get]`, `#[any]`, …).
- `#[pre_guard]`, `#[middleware]` and `#[layer]` are not supported.
  `#[guard]` and `#[intercept]` work.

At runtime:

- Two registered controllers that both declare a `#[fallback]` make the router
  build panic. An app has exactly one fallback.
- Declared routes always win. A known path called with the wrong method returns
  **405**, not the fallback.
- With the `NormalizePath` plugin, the trailing slash is stripped before
  routing, so the fallback sees the normalized path.

## Raw requests and streaming responses

The raw `Request` consumes the body, so it must be the **last** parameter.
Responses can stream:

```rust
use r2e::http::Body;

// Pass-through: reuse the incoming body as the response body, no buffering
Response::builder().status(200).body(Body::new(req.into_body())).unwrap()

// Generated stream
Body::from_stream(some_try_stream_of_bytes)

// Redirect to a presigned URL
Redirect::temporary(&url).into_response()
```

## OpenAPI

`#[any]` routes, `#[fallback]` routes and any route with a `{*wildcard}` are
left out of the OpenAPI spec. They have no single method or shape to document,
and `{*path}` is not a valid OpenAPI path template.

## Escape hatches

Pick the first option that fits. Each step down gives up framework integration
for more control:

| Level | API | You keep | You lose |
|---|---|---|---|
| 1 | `#[any]` / `#[fallback]` | DI, guards, interceptors, `TestApp` | nothing |
| 2 | `merge_router(routes)` / `register_routes(routes)` | app state, global plugins (CORS, tracing, errors) | controller DI, guards, interceptors |
| 3 | `with_layer_fn(\|router\| ..)` | the final router, to add layers or nest apps | the `State` extractor (runs after state is applied) |
| 4 | `r2e::http::axum_compat` | the raw axum API | R2E's API stability promise |

## Testing

```rust
#[r2e::test(app = my_app::MyApp)]
async fn fallback_catches_unmatched(app: TestApp) {
    let resp = app.get("/definitely/not/a/route").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
```

A full example lives in `examples/example-app/src/controllers/proxy_controller.rs`
with its test in `examples/example-app/tests/proxy_test.rs`.

## See also

- Full guide: [docs/features/20-proxy-catch-all.md](https://github.com/plawn/r2e/blob/master/docs/features/20-proxy-catch-all.md)
- [Route Attributes](./route-attributes.md)
