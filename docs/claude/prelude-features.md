# Prelude & Feature Flags

## Prelude & HTTP Re-exports (r2e-core)

**`use r2e::prelude::*`** provides everything a developer needs — no direct `axum` imports should be necessary. The prelude includes:

- **Macros:** `Controller`, `routes`, `get`/`post`/`put`/`delete`/`patch`, `guard`, `intercept`, `roles`, `managed`, `transactional`, `consumer`, `scheduled`, `bean`, `producer`, `Bean`, `BeanState`, `Params`, `ConfigProperties`, `Cacheable`, `ApiError`, `FromMultipart` (multipart feature)
- **Core types:** `AppBuilder`, `HttpError`, `R2eConfig`, `ConfigValue`, `Plugin`, `Interceptor`, `ManagedResource`, `ManagedErr`, `Guard`, `GuardContext`, `Identity`, `PreAuthGuard`, `StatefulConstruct`, `EventSubscriber`
- **Events** (feature `events`): `EventBus` (trait), `LocalEventBus` (default in-process impl)
- **Plugins:** `Cors`, `Tracing`, `Health`, `ErrorHandling`, `DevReload`, `NormalizePath`, `SecureHeaders`, `RequestIdPlugin`
- **HTTP core:** `Json`, `Router`, `StatusCode`, `HeaderMap`, `Uri`, `Extension`, `Body`, `Bytes`
- **Extractors:** `Path`, `Query`, `Form`, `State`, `Request`, `FromRef`, `FromRequest`, `FromRequestParts`, `ConnectInfo`, `DefaultBodyLimit`, `MatchedPath`, `OriginalUri`
- **Headers:** `HeaderName`, `HeaderValue`, `Method`, plus constants: `ACCEPT`, `AUTHORIZATION`, `CACHE_CONTROL`, `CONTENT_LENGTH`, `CONTENT_TYPE`, `COOKIE`, `HOST_HEADER`, `LOCATION`, `ORIGIN`, `REFERER`, `SET_COOKIE`, `USER_AGENT`
- **Response:** `IntoResponse`, `Response`, `Redirect`, `Html`, `Sse`, `SseEvent`, `SseKeepAlive`, `SseBroadcaster`
- **Middleware:** `from_fn`, `Next`
- **Type aliases:** `ApiResult`, `JsonResult`, `StatusResult`
- **Multipart** (feature `multipart`): `Multipart`, `TypedMultipart`, `UploadedFile`, `FromMultipart`
- **WebSocket** (feature `ws`): `WebSocket`, `WebSocketUpgrade`, `Message`, `CloseFrame`, `WsStream`, `WsHandler`, `WsBroadcaster`, `WsRooms`

Additional types are available via `r2e::http::*` submodules for advanced use (e.g., `r2e::http::routing::{get, post, ...}`, `r2e::http::body::Body`).

## Feature Flags

- Validation uses `garde` crate and is always available (no feature flag). Types deriving `garde::Validate` are automatically validated when extracted via `Json<T>`.
- `#[derive(Params)]` aggregates path, query, and header params into a single DTO (BeanParam-like). Also generates `ParamsMetadata` for automatic OpenAPI parameter documentation.
- `#[transactional]` attribute (in macros) wraps a method body in `self.pool.begin()`/`commit()` — requires the controller to have an injected `pool` field. Consider using `#[managed]` instead for more flexibility.

## `dev-reload` Feature Flag

The `dev-reload` feature enables Subsecond hot-patching via Dioxus. It is **intentionally excluded from `full`** to never leak into production builds.

- **Facade:** `r2e = { features = ["dev-reload"] }` → enables `r2e-devtools` + `r2e-core/dev-reload`
- **Re-export:** `r2e::devtools::*` (contains `serve_with_hotreload`, `serve_with_hotreload_env`)
- **Macro support:** `#[r2e::main]` auto-generates `#[cfg(feature = "dev-reload")]` / `#[cfg(not(...))]` code paths when `main` has a parameter (auto-calls `setup()` by convention)
- **CLI:** `r2e dev` runs `dx serve --hot-patch` with `dev-reload` feature enabled
- **`PreparedApp<T>`** is always available (not gated) — separates app building from serving
