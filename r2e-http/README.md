# r2e-http

HTTP abstraction layer for R2E — sole owner of the `axum` dependency.

## Overview

`r2e-http` is the single gateway through which the entire R2E workspace accesses Axum. No other crate depends on `axum` directly; they import HTTP types via `r2e_core::http`, which re-exports from this crate.

This enforces a clean dependency boundary: if Axum's API changes, only this crate needs updating.

## Re-exports

Core Axum types. Frequently used items are also flattened at the crate root, so both `r2e_http::Router` and `r2e_http::routing::get` resolve:

| Module | Types |
|--------|-------|
| crate root | `Router`, `Json`, `Extension`, `Error`, `Uri`, `Bytes`, plus flattened `extract`/`header`/`response`/`body` items |
| `extract` | `Path`, `Query`, `Form`, `State`, `Request`, `FromRequestParts`, `OptionalFromRequestParts`, `ConnectInfo`, `MatchedPath`, `DefaultBodyLimit`, ... |
| `header` | `HeaderMap`, `HeaderName`, `HeaderValue`, `StatusCode`, `Method`, `Parts`, all `http::header` constants |
| `response` | `Response`, `IntoResponse`, `Html`, `Redirect`, `Sse`, `SseEvent`, `SseKeepAlive` |
| `routing` | `MethodRouter`, `Route`, `get`, `post`, `put`, `patch`, `delete`, `any` |
| `body` | `Body`, `to_bytes` |
| `middleware` | `from_fn`, `from_fn_with_state`, `Next` |

The `labels` module (always compiled) provides bounded telemetry label helpers (`method_label`, `UNMATCHED_PATH_LABEL`, ...) shared by `r2e-prometheus` and `r2e-observability`.

## Feature flags

| Feature | Description |
|---------|-------------|
| `ws` | WebSocket support — `ws` module (`WebSocket`, `WebSocketUpgrade`, `Message`, `CloseFrame`) |
| `multipart` | Multipart form / file upload support — `multipart` module (`Multipart`) |
| `quic` | HTTP/3 via h3 + h3-quinn, plus raw QUIC streams (quinn) — `quic` module (`QuicEndpoint`, `QuicConnection`, `QuicError`, `build_server_config`, `apply_alt_svc`, re-exported `quinn`) |

Default features: none.

## Usage

Most users should never depend on `r2e-http` directly. Use the `r2e` facade crate or access types through `r2e_core::http`.

## License

Apache-2.0
