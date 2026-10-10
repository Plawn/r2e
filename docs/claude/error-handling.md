# Error Handling & Managed Resources

## HttpError (r2e-core)

R2E provides `HttpError` as a default error type, `#[derive(ApiError)]` for custom error types, and automatic validation error handling via garde integration. `HttpError` implements `std::error::Error`, `Clone`, `Display`, and `IntoHttpResponse` (R2E's response contract) — plus the bridging `IntoResponse` impl, so it stays returnable from a handler.

### `HttpError` variants

The enum is `#[non_exhaustive]` — always include a wildcard arm when matching.

| Variant | Status | Body |
|---------|--------|------|
| `NotFound(Cow<'static, str>)` | 404 | `{"error": "..."}` |
| `Unauthorized(Cow<'static, str>)` | 401 | `{"error": "..."}` |
| `Forbidden(Cow<'static, str>)` | 403 | `{"error": "..."}` |
| `BadRequest(Cow<'static, str>)` | 400 | `{"error": "..."}` |
| `Internal(Cow<'static, str>)` | 500 | `{"error": "..."}` |
| `Validation { status, response: ValidationErrorResponse }` | `status` (400 via `HttpError::validation(..)`) | `{"error": "Validation failed", "details": [...]}` |
| `Custom { status, body }` | any | custom JSON body |
| `WithSource { status, message, source }` | any | `{"error": "..."}` (source never exposed to client) |

### Convenience constructors

```rust
HttpError::not_found("User not found")     // zero-alloc with static strings
HttpError::internal(format!("DB: {e}"))     // accepts String too
HttpError::bad_request("invalid input")
HttpError::unauthorized("no token")
HttpError::forbidden("access denied")
HttpError::from_status(StatusCode::CONFLICT, "already exists")
```

### Accessor methods

```rust
let err = HttpError::not_found("gone");
err.status()   // StatusCode::NOT_FOUND
err.message()  // Some("gone")
```

### Adding context

```rust
// Method on HttpError
let err = HttpError::internal("connection refused").context("inserting user");
// → "inserting user: connection refused"

// Extension trait on Result<T, E: Into<HttpError>>
use r2e_core::HttpErrorExt;
let user = db.insert(&user).await.http_context("inserting user")?;
```

### Validation errors (`HttpError::Validation`)

Produced automatically by the `garde` integration. When `Json<T>` is extracted and `T: garde::Validate`, validation runs before the handler body. On failure, a 400 response is returned:
```json
{"error": "Validation failed", "details": [{"field": "email", "message": "not a valid email", "code": "validation"}]}
```
The underlying types: `ValidationErrorResponse { errors: Vec<FieldError> }` and `FieldError { field, message, code }` (in `r2e-core::validation`). The validation uses an autoref specialization trick (`__AutoValidator` / `__DoValidate` / `__SkipValidate`) so types without `Validate` have zero overhead.

### Using the built-in `HttpError`

```rust
use r2e_core::HttpError;

#[get("/{id}")]
async fn get(&self, Path(id): Path<i64>) -> Result<Json<User>, HttpError> {
    let user = self.service.find(id).await
        .ok_or_else(|| HttpError::not_found("User not found"))?;
    Ok(Json(user))
}
```

### `map_error!` macro

Bulk `From<E> for HttpError` generation:
```rust
r2e_core::map_error! {
    sqlx::Error => Internal,
    r2e_core::json::JsonError => BadRequest,
}
```

Map to a custom error type:
```rust
r2e_core::map_error! {
    for MyError {
        sqlx::Error => DbError,
    }
}
```

### Decision guide: `HttpError` vs `#[derive(ApiError)]`

| Use case | Recommended |
|----------|-------------|
| Quick prototyping, simple handlers | `HttpError` directly |
| Multiple error sources (DB, IO, parsing) | `#[derive(ApiError)]` with `#[from]` |
| Need to preserve error chain / source | `#[derive(ApiError)]` with `#[from]` |
| Wrapping `HttpError` in a larger enum | `#[derive(ApiError)]` with `#[error(transparent)]` |
| One-off status code (e.g., 429, 418) | `HttpError::from_status()` or `HttpError::Custom` |

### `#[derive(ApiError)]` (recommended for custom error types)

Generates `Display`, `IntoHttpResponse` (plus the bridging `IntoResponse` impl), and `std::error::Error` impls automatically. Available in the prelude.

```rust
#[derive(Debug, ApiError)]
pub enum MyError {
    #[error(status = NOT_FOUND, message = "User not found: {0}")]
    NotFound(String),

    #[error(status = INTERNAL_SERVER_ERROR)]
    Io(#[from] std::io::Error),

    #[error(status = BAD_REQUEST)]
    Validation(String),

    #[error(status = CONFLICT)]
    AlreadyExists,

    #[error(status = 429, message = "Too many requests")]
    RateLimited,

    #[error(status = BAD_REQUEST, message = "Field {field} is invalid: {reason}")]
    InvalidField { field: String, reason: String },

    #[error(transparent)]
    Http(#[from] HttpError),
}
```

Attribute syntax on variants:
- `#[error(status = NAME, message = "...")]` — explicit status + message with `{0}`/`{field}` interpolation
- `#[error(status = NAME)]` — status only; message inferred (String field value, `#[from]` source `.to_string()`, or humanized variant name for units)
- `#[error(status = 429)]` — numeric status code (validated at compile time: must be 100-599)
- `#[error(transparent)]` — delegates Display + IntoResponse to the inner type
- `#[from]` on a field — generates `From<T>` impl and `Error::source()` returns that field

### Manual custom error types

Alternatively, implement **`IntoHttpResponse`** manually (match variant →
`(StatusCode, Json)` tuple) and emit the backend bridge with one line:

```rust
impl IntoHttpResponse for MyError {
    fn into_http_response(self) -> Response { /* … */ }
}

r2e::http::impl_into_response!(MyError);   // non-generic types only
```

`IntoHttpResponse` is R2E's own trait; `impl_into_response!` emits the axum
`IntoResponse` impl that makes `Result<T, MyError>` composable in handlers.
Implementing axum's `IntoResponse` directly still works, but couples the type to
the HTTP backend (see `plans/runtime-http-dependency-containment.md` §5.3b).

### Error middleware patterns

**Adding request IDs to error responses:**
```rust
use r2e_core::http::middleware::{from_fn, Next};

async fn error_enrichment(req: Request, next: Next) -> Response {
    let request_id = req.extensions().get::<RequestId>().cloned();
    let mut resp = next.run(req).await;
    if let Some(id) = request_id {
        resp.headers_mut().insert("x-request-id", id.as_str().parse().unwrap());
    }
    resp
}
```

**Panic capture is always on** (no plugin needed): the catch-panic layer is installed twice by `build_inner`, carrying the `ErrorProjector` (`build_inner` reads the bean once, `ErrorProjector::default()` = `HttpError`; the 500 is `Rejection::internal("Internal server error")` projected, byte-equal to the old static body by default), and the slot that matters is the *innermost* one, below every `add_layer` — so a handler panic becomes a 500 that travels the tracing/metrics response path (summary line, RED series, `x-request-id`) instead of unwinding past it, and the `error` event it emits on target `r2e::panic` is inside the request span, hence correlated by `request_id`. The payload is downcast (`&'static str` / `String`, else `<non-string panic payload>`); backtraces are left to the `std` hook. `AppBuilder::on_panic(|report| …)` is the counting seam — R2E increments no metric itself. The outer slot is a bare net for panics raised by the outer layers; it never fires for a handler panic, so one panic = one error line. See `r2e-core/src/runtime/panic.rs` and `r2e-core/tests/http/panic.rs`.

**The hook is unified across origins (#1027):** `PanicReport::origin()` returns `PanicOrigin::Http { route }` / `Scheduled { task }` / `Executor { job }` — HTTP handlers, `#[scheduled]` ticks (via the pool's `submit_scheduled` tag; the scheduler driver is NOT a reporter) and `PoolExecutor` jobs (`submit` = `job: None`, `submit_named`/`#[async_exec]` = the method name) all reach the same hook, once per panic, with exactly one `r2e::panic` line each (field `route`/`task`/`job` matches the origin). `label()` is the bounded per-origin metric label (route template / task name / job name or `<unnamed>`); `route()`/`route_label()` stay HTTP-oriented (`route()` is `None` off the HTTP stack). Storage is a set-late `PanicHookSlot`: the builder mints it, `BeanRegistry` carries a clone into every `PluginBuildContext` (`panic_hook_slot()`), the HTTP layers resolve it at `build()` and the executor reads it at panic time — so `on_panic` works regardless of ordering vs `build_state()`, and it is stored once (not per worker). Everything else — `report_caught_panic` seam, per-origin tests — in `r2e-executor/tests/panic.rs` and `r2e-scheduler/tests/driver_edge_test/panics.rs`.

**Framework 404/405 (#1072 P4):** `layers::framework_fallbacks(app, &projector)` runs in `build_inner` after every route, controller `#[fallback]` and Routes-stage plugin router is merged and before the layers. It installs `Router::fallback` → `Rejection::not_found("Not found")` only when `r2e_http::routing::has_custom_fallback(&app)` is false (axum keeps that bit private and merging two custom fallbacks panics; the helper reads it from `Router`'s `Debug` output — the last `default_fallback:` field — and `r2e-http/tests/routing.rs` pins it against the owned axum version), and `method_not_allowed_fallback` → `RejectionKind::MethodNotAllowed` (405; axum still appends `Allow`; a route's own custom method fallback is kept). 413 needs no wiring: `Json`'s `PayloadTooLarge` rejection already projects per route (P1). Tests: `r2e-core/tests/http/fallback.rs`.

**Automatic 5xx logging:** there is no `ErrorHandling` plugin any more (removed with #1017 — panic capture is part of the router assembly). For custom 5xx logging, add a middleware layer that inspects response status codes.

**Key files:** `r2e-core/src/error/mod.rs` (HttpError, `error_response()`, `map_error!`, `HttpErrorExt`), `error/rejection.rs` (`Rejection`, `RejectionKind`, every `From<X>`), `error/schema.rs` (`ErrorSchema`, `ErrorSchemaInfo`), `error/projection.rs` (`ErrorProjector`, autoref probe), `r2e-macros/src/derives/api_error_derive.rs` (derive implementation), `r2e-core/tests/http/api_error.rs` (comprehensive tests)

---

## Rejection, envelopes, projection (#1072)

Every failure the framework raises before, around or instead of the handler is one typed value, `Rejection { kind: RejectionKind, status, message, details, headers, source }` (`r2e-core/src/error/rejection.rs`). Faults convert with plain `From` in the crate that owns them: the axum `Json`/`Path`/`Query`/`Form` rejections, `ParamError` (by `location`), `HttpError`, `GuardError`, `MultipartError`, `SecurityError` (+ `WWW-Authenticate`), `RolesDenied`, `RateLimited` (+ `Retry-After`), `FgaDenied`, `TenantError::into_rejection(statuses)`, garde `Report`, a raw `Response` (→ `Opaque`). `RejectionKind::default_status()` is the single status table (`MethodNotAllowed` 405 included since P4); `from_status` is its inverse.

An **envelope** `E: From<Rejection> + IntoHttpResponse + ErrorSchema` is what a rejection is projected into. `HttpError` is the default and its bodies are byte-equal to 0.4. `ErrorSchema` (`error/schema.rs`) is the static side — `status_of(kind)`, `body_schema[_for]`, `extra_statuses`, `opaque_passthrough` — read by `Rejection::project::<E>()` at runtime (status remapped *before* `E::from`) and by `r2e-openapi` for the spec, so runtime and spec agree for conforming envelopes (a `From<Rejection>` that renders `rejection.status`) and the documented inference — limits: the panic body is always the app envelope's, `Internal` covers unenumerable failures, colliding component names are inlined + warned. `#[derive(ApiError)]` with one `#[error(rejection)]` variant emits both impls; an enum whose only framework link is `#[error(transparent)] Http(#[from] HttpError)` inherits them.

**Which envelope renders a route** (`error/projection.rs`): `#[routes]` probes the handler's return type by autoref (`ProjectionProbe` → `ProjectEnvelope` when `Result<T, E>` qualifies, else `ProjectFallback`). The probe never constrains `T`, so a `Result<impl Trait, E>` (an opaque type cannot be named in the generated probe) is probed as `Result<(), E>` when `E` itself is nameable — the route keeps its envelope at runtime *and* in `__r2e_meta` (both come from `Projection::for_signature` in `r2e-macros/src/codegen/handlers.rs`); only a bare `impl Trait` return falls back to the app projector. The fallback is the app-level `ErrorProjector` bean — `AppBuilder::error_projection::<E>()` = `provide(ErrorProjector::of::<E>())`, `Default` = `HttpError` — the JAX-RS `ExceptionMapper` equivalent. It also renders SSE/WS routes, the catch-panic 500, the router 404/405 and (through `Json`'s `PayloadTooLarge`) the 413. There is deliberately **no** `#[error(E)]` / `#[routes(error = E)]` attribute and no `IntoRejection`-style trait (user decision 2026-10-09 — a route declared infallible is a definition R2E does not fix for the developer; don't re-propose). Design and phase record: `plans/error-projection.md`; developer migration: `docs/migration/error-projection.md`.

**Rejection-kind inference** (`static_rejection_kinds` + `rejection_kinds_expr` in `r2e-macros/src/codegen/controller_impl.rs`, → `RouteInfo::rejection_kinds`): the route, SSE and WS metadata read the **same** parameter list as the entry fn (`handlers::RequestParams::{route,sse,ws}` — managed/identity/WS-upgrade params excluded, last param consumes the body except on WS), so `Query`/`Path`/`Form`/`#[derive(Params)]`/garde kinds are documented on streaming routes too. Identity: any `#[inject(identity)]` parameter **or** struct field — required or `Option<..>` (an invalid token on an optional identity still answers 401) — adds `Unauthenticated`, except under `#[anonymous]`. `Form<T>`: on GET/SSE (query string) → `InvalidForm` (400) — WS endpoints extract request parts only, so `Form<T>` does not compile there; on a body method → `UnsupportedMediaType`/`PayloadTooLarge`/`BodyRead`/`InvalidBody` (422; `From<FormRejection>` maps `FailedToDeserializeFormBody` → `InvalidBody`, `FailedToDeserializeForm` → `InvalidForm`, the carried status always survives); `#[any]` gets both. `Internal` is pushed **unconditionally**: managed resources, `#[inject(request)]` extractors and a route envelope that does not match an extractor's own failure cannot be enumerated at macro time. The OpenAPI builder (`error_responses` in `r2e-openapi/src/builder.rs`) adds the panic 500 from the **app** `ErrorSchemaInfo` (the catch-panic layer renders with the `ErrorProjector`, not the route envelope), and error body schemas go through the same `$defs` promotion as every other component (`promote_defs`). Bodies dedup by **schema equality** (first-recorded name wins, route envelope before app); distinct bodies on one status → `anyOf` (a validation body also matches the plain body, so `oneOf` would be wrong). `build_spec_with_warnings` collects bodies per route first, fills component slots only after the route/registry schemas and their `$defs` (routes' own envelopes before the app's, `or_insert`), then emits each body as a `$ref` when its slot holds its schema and **inline** otherwise, with a `SchemaGap::ErrorBodyInlined` warning. Overrides apply after the slot snapshot, so they never turn a `$ref` inline. Known gap: a collision on a nested `$defs` type *inside* an error body is not detected (same `or_insert` behavior as route `$defs`).

Order inside the generated entry fn (one per route, SSE and WS endpoint): pre-auth guards → request data (identity + `#[inject(request)]`, via `RequestData<S>`) → guards → head parameters → body (last) → garde → managed acquire → interceptors → handler → managed finalize. Each `Err(x)` is `Rejection::from(x)` then `project::<E>()`, once; the body is never read before identity and guards pass. **Header merge** (`merge_headers` in `rejection.rs`, shared by the rendered and the opaque-passthrough paths): per header *name*, a name the rendered/opaque response already carries wins and every hub value for it is dropped; otherwise every hub value is appended (two `WWW-Authenticate` challenges both survive). **Guard scoping is uniform across HTTP, gRPC and MCP**: controller-level guards are built once per controller and run with `GuardContext { method_name: "*" }`, method guards with the method name — so a controller-level `RateLimit` is one service-wide budget, never charged twice per call. `HttpError::Validation { status, response }` carries its status so an envelope's `status_of(Validation)` remap survives `From<Rejection> for HttpError` and back. Wires: `r2e_grpc::rejection_to_status` (free fn — `From<Rejection> for tonic::Status` would be an orphan impl) and `McpError: From<Rejection>` map by kind. Tests: `r2e-core/tests/http/{rejection,projection,fallback,panic}.rs`, `tests/controller/error_meta.rs`, `r2e-openapi/tests/errors.rs`, `r2e-mcp/tests/server/rejection.rs`, `r2e-grpc/tests/guard.rs`.

---

## Guards (error helpers)

The `GuardError` struct simplifies guard error construction; it converts `Into<Rejection>`, so `.into()` / `?` produce the `Rejection` a `check -> Result<(), Rejection>` returns (the status is kept, the kind is derived from it):

```rust
use r2e_core::decorators::guards::GuardError;

// `.into()` yields the `Rejection` the route projects through its envelope:
Err(GuardError::forbidden("Insufficient permissions").into())
Err(GuardError::unauthorized("Missing API key").into())
Err(GuardError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited").into())
```

---

## Managed Resources (r2e-core)

The `#[managed]` attribute enables automatic lifecycle management for resources like database transactions, connections, scoped caches, or audit contexts. Resources are acquired before handler execution and released after, with success/failure status.

### Core trait

```rust
pub trait ManagedResource<S>: Sized + Send {
    type Error: Into<Rejection>;

    fn acquire(
        context: ManagedContext<'_, S>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;
    fn finalize(
        &mut self,
        outcome: &ManagedOutcome,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn abort(&mut self);
}

pub trait ManagedDeps {
    type Deps;
}
```

Every managed type must implement `ManagedDeps`. Its type-level list names the
beans that `acquire` looks up, so missing beans fail at controller registration.
There is no blanket implementation; a request/state-only resource declares
`TNil` explicitly:

```rust
use r2e_core::{
    HttpError, ManagedContext, ManagedDeps, ManagedErr, ManagedOutcome,
    ManagedResource, TNil,
};

struct TenantAudit {
    tenant: String,
}

impl<S: Send + Sync> ManagedResource<S> for TenantAudit {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let head = context.require_request()?;
        let tenant = head
            .header("x-tenant")
            .ok_or_else(|| ManagedErr(HttpError::bad_request("tenant missing")))?;
        Ok(Self { tenant: tenant.to_owned() })
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

impl ManagedDeps for TenantAudit {
    type Deps = TNil; // acquire reads no bean
}
```

### Usage with `#[managed]`

```rust
#[routes]
impl UserController {
    #[post("/")]
    async fn create(
        &self,
        body: Json<User>,
        #[managed] tx: &mut Tx<'_, Sqlite>,
    ) -> Result<Json<User>, MyHttpError> {
        sqlx::query("INSERT INTO users ...").execute(tx.as_mut()).await?;
        Ok(Json(user))
    }
}
```

### Lifecycle

1. `acquire(context)` — called before handler, resource obtained from app state
2. Handler receives `&mut Resource`
3. Build the HTTP response and call `finalize(&outcome)` in reverse order
4. Call `abort()` from the RAII guard on panic, cancellation, partial acquire,
   or failed finalization

The SQLx and Diesel backend crates provide transaction implementations. They
resolve pool beans by type, commit responses below 400, roll back `4xx`/`5xx`,
and use a drop-safe abort fallback.

**Note:** `#[managed]` is the only transaction attribute — the legacy
`#[transactional]` body wrapper was removed (W10 phase 4).

### Error wrappers for `ManagedResource`

`ManagedErr<E>` — generic wrapper for any `Into<Rejection>` type. Needed because orphan rules prevent `impl Into<Rejection> for YourError` directly. Use `ManagedErr<HttpError>` for the common case. Acquire and finalize failures are projected through the route's error envelope like every other rejection.
