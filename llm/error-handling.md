---
topic: error-handling
features: core
tokens: ~4200
requires: core-concepts
---

## Error Handling

### TL;DR

- `HttpError` is the default error type: `HttpError::not_found/bad_request/internal/from_status`, plus `.http_context("...")` (from `HttpErrorExt`) to add context to a `?`.
- `HttpError` is `#[non_exhaustive]` — always include a wildcard arm when matching it.
- Prefer `#[derive(ApiError)]` for custom error enums: `#[error(status = ..., message = "...")]`, `#[from]` for sources, `#[error(transparent)]` to wrap `HttpError`.
- For a hand-written response type implement `IntoHttpResponse` and emit the bridge with `r2e::http::impl_into_response!(Ty)` (non-generic types only) — never implement axum's `IntoResponse` yourself.
- Constant bodies use `r2e::http::response::static_json(status, r#"..."#)`; anything holding a runtime value must go through `Json` / `json!` for escaping.
- `r2e::map_error! { for MyError { Err => Variant, ... } }` generates the `From`
  impls so `?` converts into your error type; the bare `{ Err => Variant }` form
  targets `HttpError` and is orphan-rule-illegal outside `r2e-core` — convert at
  the call site with `.map_err(|e| HttpError::internal(e.to_string()))` instead.
- Framework failures (extractor rejections, guard denials, garde reports, …) are
  typed as one hub value, `Rejection { kind: RejectionKind, status, message,
  details, headers, source }`. Every fault type converts with plain `From`;
  `HttpError: From<Rejection>` renders the default bodies; your own envelope
  implements `From<Rejection> + IntoHttpResponse + ErrorSchema` — or gets all
  three from `#[derive(ApiError)]` with one `#[error(rejection)]` variant.
- The envelope that renders a route is inferred from its return type:
  `Result<T, E>` projects every framework failure on that route through `E`;
  any other return type uses `AppBuilder::error_projection::<E>()` (default
  `HttpError`), which also renders the framework's own 404 / 405 / 413 and the
  panic 500. There is no attribute to write.
- Panics are caught automatically (no plugin): JSON 500, plus one `error` event
  on target `r2e::panic` inside the request span (so `request_id` + `route`).
  Count them with `.on_panic(|report| ...)` — one hook for HTTP handlers,
  `#[scheduled]` ticks and executor jobs (`report.origin()` / `report.label()`).

### HttpError (built-in)

`HttpError` is the default error type (`#[non_exhaustive]` — wildcard arm when
matching). Variants: `NotFound`, `Unauthorized`, `Forbidden`, `BadRequest`,
`Internal` (all `Cow<'static, str>`), `Validation { status, response }` (build with `HttpError::validation(resp)` → 400), `Custom { status, body }`,
`WithSource { status, message, source }`.

```rust
# async fn __doc(db: Db, u: NewUser, e: std::io::Error) -> Result<(), HttpError> {
HttpError::not_found("User not found");     // zero-alloc with static strings
HttpError::internal(format!("DB: {e}"));
HttpError::bad_request("invalid input");
HttpError::from_status(StatusCode::CONFLICT, "already exists");

// context
let user = db.insert(&u).await.http_context("inserting user")?;  // via HttpErrorExt
# Ok(()) }
```

### `#[derive(ApiError)]` — custom error types (recommended)

Generates `Display`, `IntoHttpResponse` (plus the `IntoResponse` bridge impl,
so the type is returnable from a handler), and `std::error::Error`:

```rust
#[derive(Debug, ApiError)]
pub enum MyError {
    #[error(status = NOT_FOUND, message = "User not found: {0}")]
    NotFound(String),

    #[error(status = INTERNAL_SERVER_ERROR)]
    Io(#[from] std::io::Error),          // From impl + Error::source()

    #[error(status = 429, message = "Too many requests")]
    RateLimited,

    #[error(transparent)]
    Http(#[from] HttpError),
}
```

### `Rejection` — the typed framework failure

Everything the framework rejects before or around your handler — a malformed
JSON body, a missing `content-type`, a bad path segment, a failed JWT, a denied
guard, a garde report, a rate limit — is one value: `Rejection` (prelude).

```rust
# fn __doc(r: Rejection) {
let kind: RejectionKind = r.kind;        // closed enum, `#[non_exhaustive]`
let status: StatusCode = r.status;       // starts at `kind.default_status()`
let message: &str = &r.message;          // client-facing; `Display` prints it
let details = r.details.as_ref();        // `Option<serde_json::Value>` — garde field errors
let headers = &r.headers;                // `WWW-Authenticate`, `Retry-After`, …
# }
```

`RejectionKind::default_status()` is the one status table: `MissingContentType`
/ `UnsupportedMediaType` 415, `PayloadTooLarge` 413, `BodyRead` / `MalformedBody`
/ `InvalidPath` / `InvalidQuery` / `InvalidForm` / `InvalidHeader` / `BadRequest`
/ `Validation` 400, `InvalidBody` 422 (deserialized but semantically wrong),
`Unauthenticated` 401, `Forbidden` 403, `NotFound` 404, `MethodNotAllowed` 405,
`Conflict` 409,
`RateLimited` 429, `Internal` 500, `Unavailable` 503, `Timeout` 504, and
`Opaque` for a pre-rendered `Response` the framework could not type.

Faults reach the hub through std `From`, implemented in the crate that owns the
fault: `JsonRejection` / `PathRejection` / `QueryRejection` / `FormRejection`,
`ParamError` (by `location`), `HttpError`, `GuardError` (kind from the status,
status kept), `SecurityError` (401 + `WWW-Authenticate: Bearer`, or 503 for a
JWKS failure), `RolesDenied` (r2e-security), `RateLimited` (r2e-rate-limit,
sets `Retry-After`), `FgaDenied` (r2e-openfga), `TenantError` (r2e-tenant; use
`into_rejection(statuses)` to keep configured statuses), garde `Report`,
`Response` (→ `Opaque`). Build one directly with `Rejection::new(kind, msg)`,
`Rejection::with_status(kind, status, msg)`, `Rejection::from_status(status, msg)`
or the shortcuts `unauthenticated()` / `forbidden(msg)` / `not_found(msg)` /
`bad_request(msg)` / `internal(msg)`, then `.details(value)` /
`.header(name, value)` / `.source(err)`.

### Error envelopes — `From<Rejection> + IntoHttpResponse + ErrorSchema`

An **envelope** is the type a rejection is projected into before it is
rendered. `HttpError` is the default and reproduces the plain bodies
(`{"error": msg}`, `{"error":"Validation failed","details":[…]}`). A foreign
wire shape (OpenAI-style `{"error":{"type":…}}`, RFC 9457 problem details, …)
is an envelope `E` with three impls:

- `From<Rejection>` — build the body from `r.kind` / `r.message` / `r.details`,
  and **read `r.status`**, never the table: the status may have been remapped or
  carried by the fault (a 413 body read, a configured tenant status).
- `IntoHttpResponse` (+ `impl_into_response!`), as for any response type.
- `ErrorSchema` — the static side, read by the runtime **and** the OpenAPI
  builder so the spec cannot say 422 where the server answers 400:

```rust
pub trait ErrorSchema {
    /// Status for `kind`. Default: `kind.default_status()`. A kind left at its
    /// default keeps whatever status the fault carried.
    fn status_of(kind: RejectionKind) -> StatusCode { kind.default_status() }
    /// `(component name, JSON Schema)` of the body; `None` = undocumented.
    fn body_schema() -> Option<(String, serde_json::Value)>;
    /// Per-kind body when one envelope has several shapes.
    fn body_schema_for(_kind: RejectionKind) -> Option<(String, serde_json::Value)> { None }
    /// Statuses the envelope emits that no inferred kind covers (502 for a proxy).
    fn extra_statuses() -> Vec<(StatusCode, &'static str)> { Vec::new() }
    /// Return an `Opaque` rejection untouched instead of re-wrapping it (`HttpError`: true).
    fn opaque_passthrough() -> bool { false }
}
```

`rejection.project::<E>()` is the one projection step: it applies
`E::status_of(kind)` when that differs from the default, honours
`opaque_passthrough`, calls `E::from(rejection).into_http_response()` and adds
the hub headers (the envelope's own header of the same name wins).

The quickest envelope is a derive: mark **one** variant `#[error(rejection)]`
over a single `Rejection` field (or `#[error(transparent)]` over a `Rejection`
field, which means the same). The derive then also emits `From<Rejection>` and
`ErrorSchema` — `status_of` is the default table, `body_schema` delegates to
`HttpError`'s, `extra_statuses` lists the other variants' fixed statuses, and
`opaque_passthrough()` is `true`. The variant takes no `status`/`message`;
remap statuses by implementing `ErrorSchema` by hand instead.

```rust
#[derive(Debug, ApiError)]
pub enum ApiEnvelope {
    #[error(status = CONFLICT, message = "already exists: {0}")]
    Duplicate(String),

    #[error(rejection)]                 // From<Rejection> + ErrorSchema come with it
    Rejected(Rejection),
}

fn __assert<E: From<Rejection> + IntoHttpResponse + ErrorSchema>() {}
# fn __doc() {
__assert::<ApiEnvelope>();
__assert::<HttpError>();               // the default envelope
# }
```

An enum whose only link to the framework is `#[error(transparent)] Http(#[from] HttpError)`
inherits the same two impls through `HttpError`.

### Which envelope renders a route

There is no attribute to wire an envelope: the **handler's return type** decides.

- `Result<T, E>` with `E: From<Rejection> + IntoHttpResponse + ErrorSchema` —
  every framework failure on that route (extractor rejection, failed identity,
  guard denial, garde report, managed acquire/finalize) is projected through
  `E`, so the handler's own `Err(E)` and the framework's errors share one wire
  shape. Nothing to annotate. `HttpError` qualifies, so a `Result<T, HttpError>`
  route renders `HttpError` bodies **regardless** of the app-level projection.
  `T` is never constrained: `Result<impl IntoResponse, E>` keeps `E` too (the
  probe uses `Result<(), E>` when the success type is an opaque `impl Trait`).
- Any other return type (`Json<T>`, a bare `impl IntoResponse`, `Result<T, E>`
  with `E` lacking one of the three traits, a plain `String`) — the route uses
  the **app-level** projection: `AppBuilder::error_projection::<E>()`, default
  `HttpError`. The app-level one is the JAX-RS/Quarkus `ExceptionMapper`
  equivalent: one place that decides how unmapped failures look.

A route declared infallible (`-> Json<T>`) is a definition the framework trusts:
its failures still exist (a bad body, a missing identity) and render with the
app-level envelope. Declare `Result<T, E>` when the route must speak `E`.

```rust
# use r2e::ErrorProjector;
#[derive(Debug, ApiError)]
pub enum ApiEnvelope {
    #[error(status = CONFLICT, message = "already exists: {0}")]
    Duplicate(String),
    #[error(rejection)]
    Rejected(Rejection),
}

#[controller(path = "/items")]
pub struct ItemController;

#[routes]
impl ItemController {
    #[post("/")]
    async fn create(&self, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiEnvelope> {
        Ok(Json(body))                     // a malformed body answers as `ApiEnvelope` too
    }

    #[get("/{id}")]
    async fn get(&self, Path(id): Path<u32>) -> Json<u32> {
        Json(id)                           // a bad `id` answers with the app-level envelope
    }
}

# async fn __doc() {
let state = AppBuilder::new()
    .error_projection::<ApiEnvelope>()     // app-level envelope (default: HttpError)
    .build_state()
    .await;
let projector = r2e::BeanAccess::get::<ErrorProjector>(state.state());
let _resp = projector.project(Rejection::not_found("gone"));
# }
# fn main() {}
```

`error_projection::<E>()` provides an `ErrorProjector` bean (`project(rejection)
-> Response`); plugins and hand-written handlers can inject it to render a
`Rejection` with the app's envelope. The framework's own responses go through
it too: a panicking handler (500, kind `Internal`), an unknown route (404,
`NotFound` — unless the app installed a fallback of its own: a controller
`#[fallback]`, a merged `Router::fallback(..)`, the static SPA fallback), a
known path with the wrong method (405, `MethodNotAllowed`, `Allow` kept) and
an oversized body (413, `PayloadTooLarge`). With the default envelope these
are `{"error":"Not found"}`, `{"error":"Method not allowed"}`, and so on.
`E::status_of` applies to them like to any other kind. The runtime order on
every route is fixed:
pre-auth guards → identity + `#[inject(request)]` fields → guards → path/query/
header parameters → body → garde validation → managed acquire → interceptors →
handler → managed finalize. A failure at any step is projected once, through
the route's envelope, and the body is never read before identity and guards
have passed.

### Hand-written response types

When `#[derive(ApiError)]` does not fit, implement **`IntoHttpResponse`** (R2E's
contract) and emit the backend bridge with one macro line — do NOT implement
axum's `IntoResponse` yourself:

```rust
use r2e::prelude::*;                       // IntoHttpResponse, Response, ...

#[derive(Debug)]
pub struct Conflict;

impl IntoHttpResponse for Conflict {
    fn into_http_response(self) -> Response {
        (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "conflict" })))
            .into_response()
    }
}

r2e::http::impl_into_response!(Conflict);  // bridge; non-generic types only
```

`impl_into_response!` is what keeps handler composition working: `Result<T, E>`
and `(StatusCode, T)` reach the HTTP backend through it.

For a **constant** body, skip `json!` entirely — it allocates a `Value` and runs
the serializer on every response:

```rust
use r2e::http::response::static_json;

# fn __doc() -> Response {
static_json(StatusCode::UNAUTHORIZED, r#"{"error":"Unauthorized"}"#)
# }
```

`static_json` sets `content-type: application/json` and sends the `&'static str`
as `Bytes::from_static` (zero-copy). Only for literal constants — a body holding
a runtime value must keep going through `Json`/`json!` for escaping.

### `map_error!` — bulk From impls

`map_error!` writes the `From` impls that make `?` convert. In an application
crate use the `for <YourError>` form: the bare form targets `HttpError`, and an
`impl From<sqlx::Error> for HttpError` written in your crate is an orphan-rule
error (both types are foreign), so that form only compiles inside `r2e-core`.

```rust
#[derive(Debug, ApiError)]
pub enum MyError {
    #[error(status = INTERNAL_SERVER_ERROR, message = "{0}")]
    Internal(String),

    #[error(status = BAD_REQUEST, message = "{0}")]
    BadRequest(String),
}

r2e::map_error! { for MyError {
    sqlx::Error => Internal,
    r2e::json::JsonError => BadRequest,
}}
// `?` on sqlx::Error now auto-converts to MyError::Internal
```

For a one-off, convert at the call site instead:
`.map_err(|e| HttpError::internal(e.to_string()))?`.

### Panics

Panic capture is always on — no plugin, no opt-in. A panicking handler answers
`500 {"error":"Internal server error"}` (`Rejection::internal` through the
app-level envelope, see above) and R2E emits **one** `error` event on
target `r2e::panic` with `panic_message` and the matched `route`. The layer sits
*below* the tracing and metrics layers, so the event is inside the request span
(it carries `request_id`) and the request still gets its `request completed`
summary line, its 5xx metric series and its `x-request-id` echo.

The payload is downcast from `&'static str` and `String`; anything else logs
`<non-string panic payload>`. Backtraces are left to the `std` panic hook.

R2E increments no metric of its own — every service owns its registry and
prefix. `AppBuilder::on_panic` is the seam, and it is **unified**: it fires
once per panic caught anywhere the framework contains one — HTTP handlers,
`#[scheduled]` ticks, and `PoolExecutor` jobs (`#[async_exec]`,
`executor.submit`) all reach the same hook. `PanicReport::origin()` says
where; `label()` is one bounded metric label for every origin:

```rust,ignore
AppBuilder::new()
    .on_panic(|report| {
        // report.message() -> &str
        // report.origin() -> PanicOrigin<'_>:
        //   Http { route: Option<&str> } | Scheduled { task: &str } | Executor { job: Option<&str> }
        // report.label() -> &str — route template (or the metrics' `unmatched`),
        //   task name, or job name (`<unnamed>`; `#[async_exec]` = method name)
        // report.route() -> Option<&str> — Some only for Http
        metrics::counter!("app_panics_total", "at" => report.label().to_owned())
            .increment(1);
    })
```

`PanicReport` / `PanicOrigin` (prelude; `PanicHook` / `PANIC_TARGET` at the
crate root) are deliberately minimal — message, origin, label, nothing
request-borne. The hook runs on the panicking task while the panic is
converted to its outcome: keep it short and non-blocking. A panic inside the
hook is caught and logged; the outcome — the JSON 500, the failed job
(`JoinError::is_panic()`), the next scheduled tick — is unchanged. Each origin
emits exactly one `r2e::panic` line, with `route`, `task`, or `job` as the
field.
