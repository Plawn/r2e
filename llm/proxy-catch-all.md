---
topic: proxy-catch-all
features: core
tokens: ~400
requires: core-concepts
---

## Proxy & Catch-All Routes

### TL;DR

- Stay in a controller: `#[any("/prefix/{*path}")]` for every method + wildcard path, `#[fallback]` for the app-wide catch-all — do NOT hand-mount an axum fallback.
- `#[fallback]` takes no path argument, and its controller must have no path prefix.
- `req: Request` (from `r2e::http::extract::Request`) must be the LAST handler parameter.
- Stream through with `Body::from_stream(...)` or `Body::new(req.into_body())`.
- Guards, interceptors, DI and TestApp all work here, but `#[any]`/`#[fallback]`/wildcard routes are excluded from OpenAPI.

For gateways/proxies, stay in controllers — do NOT hand-mount an axum fallback:

```rust
use r2e::http::extract::Request;

#[controller]
pub struct ProxyController {
    #[inject] upstream: UpstreamClient,
}

#[routes]
impl ProxyController {
    #[any("/registry/{*path}")]              // every HTTP method, wildcard path
    async fn proxy(&self, req: Request) -> Response {
        self.upstream.forward(req).await     // Request must be the LAST param
    }

    #[fallback]                              // app-wide catch-all (no path argument;
    async fn dispatch(&self, req: Request) -> Response {        // controller must have no path prefix)
        self.upstream.forward(req).await
    }
}
# fn main() {}
```

Guards, interceptors, DI, and TestApp all work on these routes. Streaming:
`Body::from_stream(...)`, or pass through `Body::new(req.into_body())`.
`#[any]`/`#[fallback]`/wildcard routes are excluded from OpenAPI.
