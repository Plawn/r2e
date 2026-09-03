---
topic: error-handling
features: core
tokens: ~1800
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
- Panics are caught automatically (no plugin): JSON 500, plus one `error` event
  on target `r2e::panic` inside the request span (so `request_id` + `route`).
  Count them with `.on_panic(|report| ...)` — one hook for HTTP handlers,
  `#[scheduled]` ticks and executor jobs (`report.origin()` / `report.label()`).

### HttpError (built-in)

`HttpError` is the default error type (`#[non_exhaustive]` — wildcard arm when
matching). Variants: `NotFound`, `Unauthorized`, `Forbidden`, `BadRequest`,
`Internal` (all `Cow<'static, str>`), `Validation(...)`, `Custom { status, body }`,
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
`500 {"error":"Internal server error"}` and R2E emits **one** `error` event on
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
