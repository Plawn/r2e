# r2e-http

HTTP abstraction layer for R2E — sole owner of the `axum` dependency.

## Overview

`r2e-http` is the single gateway through which the entire R2E workspace accesses Axum. No other crate depends on `axum` directly; they import HTTP types via `r2e_core::http`, which re-exports from this crate.

This enforces a clean dependency boundary: if Axum's API changes, only this crate needs updating.

## Re-exports

Core Axum types, grouped by module:

| Module | Types |
|--------|-------|
| `extract` | `Path`, `Query`, `Form`, `State`, `Json`, `Request`, `FromRequestParts`, `ConnectInfo`, ... |
| `header` | `HeaderMap`, `HeaderName`, `StatusCode`, `Method`, common header constants |
| `response` | `Response`, `IntoResponse`, `Html`, `Redirect`, `Sse`, `SseEvent` |
| `routing` | `Router`, `MethodRouter`, `get`, `post`, `put`, `delete`, ... |
| `body` | `Body` |
| `middleware` | Tower middleware helpers |

## Feature flags

| Feature | Description |
|---------|-------------|
| `ws` | WebSocket support |
| `multipart` | Multipart form / file upload support |
| `quic` | HTTP/3 via h3 + h3-quinn, plus raw QUIC streams |

## Usage

Most users should never depend on `r2e-http` directly. Use the `r2e` facade crate or access types through `r2e_core::http`.

## License

Apache-2.0
