---
topic: coming-from-axum
features: core
tokens: ~1300
requires: 
---

## Coming from Axum — do X, not Y

### TL;DR

- Reach for the R2E construct first; dropping to raw axum forfeits compile-time checking, DI, guards, OpenAPI and TestApp support.
- Do NOT add `axum` to `Cargo.toml` — reach every HTTP type through `r2e::http` / `r2e::prelude`.
- Implement R2E's contracts, not axum's: `IntoHttpResponse` (+ the `r2e::http::impl_into_response!` bridge) for responses, `FromRequestPartsVia` for bean-backed extractors.
- Typed JSON goes through `r2e::json` (`to_vec`, `to_string`, `from_slice`, `from_str`), never `serde_json::to_vec`; `serde_json::Value` / `json!` stay `serde_json`.
- Use the table below as the translation: identity injection instead of a custom `FromRequestParts`, a `Guard` instead of in-handler `if`, `.provide()` + `#[inject]` instead of `State<Arc<..>>`, `#[scheduled]` / `PoolExecutor` instead of `tokio::spawn`.
- When raw axum is unavoidable, climb the ladder in order: `#[any]`/`#[fallback]` → `merge_router` → `with_layer_fn` → `r2e::http::axum_compat` (off the supported surface, deliberately greppable).

R2E wraps axum; nearly every axum pattern has a framework-integrated equivalent
that gets DI, guards, interceptors, OpenAPI, and TestApp support for free.
**Reach for the R2E construct first.** Dropping to raw axum forfeits
compile-time checking and framework integration.

**What R2E promises.** The public surface is **R2E types**, under `r2e::http`
and `r2e::prelude` — that is what R2E commits to keeping working. `Json<T>` is
R2E's own type (extractor + response, `JsonRejection` on failure); many others
are axum's today (`Path`, `Router`, …), but you should reach them all through
`r2e::…` names, and implement R2E's own contracts rather than axum's:
`IntoHttpResponse` for response conversion (+ the one-line
`r2e::http::impl_into_response!` bridge), and `FromRequestPartsVia` for
bean-backed extractors. Do NOT add `axum` to your `Cargo.toml`.

**JSON codec.** Typed (de)serialization in framework and app code goes through
`r2e::json` (`to_vec`, `to_string`, `from_slice`, `from_str`, `JsonError` with
`is_data()/is_syntax()/is_eof()`), never `serde_json::to_vec` & co. `Json<T>`,
`HttpError` bodies, WebSocket/SSE payloads, the event bus and `#[derive(Cacheable)]`
all use it, so the codec is one Cargo feature: default `serde_json`, or
`json-sonic` (sonic-rs, SIMD — measure on your target first; on Apple Silicon
it only pays on larger serialized responses). `serde_json::Value` / `json!`
stay `serde_json` — they are the dynamic-tree type, not a codec call.

When you genuinely need something R2E does not re-export, use the explicit
escape hatch — `r2e::http::axum_compat` (`use r2e::http::axum_compat::axum;`).
It is deliberately greppable: importing from it couples your app to the axum
backend and steps outside the promise above. Prefer asking for a re-export from
`r2e::http` over spreading `axum_compat` imports.

| You want | Do NOT write | Write instead |
|---|---|---|
| Auth on a route | A custom `FromRequestParts` extractor | `#[inject(identity)] user: AuthenticatedUser` (struct- or param-level); custom identities via `FromValidatedJwtClaims` |
| Authorization / permission check | Middleware or in-handler `if` | A `Guard` (`#[guard(MyGuard)]`) or `#[roles("admin")]` |
| Public routes on a protected controller | Splitting the controller or optional extractors | `#[anonymous]` on the route (struct identity stays fail-closed) |
| A catch-all / proxy handler | `Router::fallback(handler)` | `#[fallback]` or `#[any("/prefix/{*path}")]` route on a controller |
| Endpoints grouped by resource | A `Router` of free functions | A `#[controller(path = "...")]` + `#[routes]` impl |
| Shared services | `State<Arc<MyState>>` + field access | `.provide(bean)` / `.register::<T>()` + `#[inject]` fields |
| Config values in handlers | Lazy statics / `std::env` | `#[config("key")]` fields, `load_config::<Root>()` + `#[inject]` typed sections |
| Cross-cutting logging/timing/caching | Tower middleware per concern | `#[intercept(Logged::info())]` / `Timed` / `Cache` |
| Request-scoped values (tenant id, trace ctx) | Extensions + middleware | `#[inject(request)]` field or param |
| Background jobs | `tokio::spawn` in main | `#[scheduled(every = "5m")]` methods, or `PoolExecutor` / `#[derive(BackgroundService)]` |
| Pub/sub between components | Channels wired by hand | `EventBus` (`.emit()`) + `#[consumer]` methods |
| Ask another component for a result | Oneshot channels / shared state | `EventBus` `.request()` / `.respond()` (or a `#[consumer]` with a return value) |
| Test the app | Re-declaring routers in tests | `#[r2e::test(app = my_app::MyApp)]` — boots the real `App` |
| A pool/client **per tenant** | A `HashMap<String, Pool>` in shared state | `#[inject(request)] db: Tenant<PgPool>` + `Tenancy`/`PerTenant` plugins (feature `tenant`) |

Escape-hatch ladder when you genuinely need raw axum (each rung trades
integration for control): 1. `#[any]`/`#[fallback]` controller routes →
2. `merge_router(router)` (raw fragment, shares state + global plugins, no DI) →
3. `with_layer_fn(|router| ...)` (transform the final `Router`) →
4. `r2e::http::axum_compat` (the raw axum API, off the supported surface).
