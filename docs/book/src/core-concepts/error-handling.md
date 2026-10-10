# Error Handling

R2E provides a built-in `HttpError` type, a `#[derive(ApiError)]` macro for custom error types, and automatic validation error handling via `garde`.

Since 0.5, every failure the framework raises around a handler (extractor
rejection, failed identity, denied guard, garde report, managed resource, panic,
404/405/413) is one typed value, `Rejection`, projected **once per route** into
an *error envelope* before it is rendered. [Rejection, envelopes and
projection](#rejection-envelopes-and-projection) explains the model; coming from
0.4, read the [0.4 → 0.5 migration guide](../reference/migration-0.5.md).

## Built-in `HttpError`

`HttpError` maps common error cases to HTTP status codes:

```rust
use r2e::prelude::*; // HttpError, Json, Path

#[get("/{id}")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, HttpError> {
    self.service.get_by_id(id).await
        .map(Json)
        .ok_or_else(|| HttpError::NotFound("User not found".into()))
}
```

### Variants

| Variant | HTTP Status | JSON body |
|---------|------------|-----------|
| `HttpError::NotFound(msg)` | 404 | `{"error": "User not found"}` |
| `HttpError::Unauthorized(msg)` | 401 | `{"error": "..."}` |
| `HttpError::Forbidden(msg)` | 403 | `{"error": "..."}` |
| `HttpError::BadRequest(msg)` | 400 | `{"error": "..."}` |
| `HttpError::Internal(msg)` | 500 | `{"error": "..."}` |
| `HttpError::Validation { status, response }` | 400 (carried `status`) | `{"error": "Validation failed", "details": [...]}` |
| `HttpError::Custom { status, body }` | any | custom JSON body |

### Custom status codes

```rust
#[post("/")]
async fn create(&self, body: Json<Request>) -> Result<Json<Response>, HttpError> {
    Err(HttpError::Custom {
        status: StatusCode::CONFLICT,
        body: serde_json::json!({
            "error": "duplicate_entry",
            "message": "A user with this email already exists",
        }),
    })
}
```

### Validation variant

`HttpError::Validation { status, response }` carries a `ValidationErrorResponse` with per-field error details plus the status it renders with (400 by default; an envelope's `status_of` remap, e.g. to 422, is kept when the hub converts into `HttpError`). This is the variant produced by automatic `garde` validation (see [Validation](./validation.md)), but you can also construct it manually with `HttpError::validation(..)` (status 400):

```rust
use r2e_core::web::validation::{ValidationErrorResponse, FieldError};

Err(HttpError::validation(ValidationErrorResponse {
    errors: vec![
        FieldError { field: "email".into(), message: "already taken".into(), code: "unique".into() },
    ],
}))
```

The JSON response:

```json
{
    "error": "Validation failed",
    "details": [
        { "field": "email", "message": "already taken", "code": "unique" }
    ]
}
```

### `map_error!` — bulk `From` impls for `HttpError`

For mapping multiple external error types to `HttpError` variants at once:

```rust
r2e_core::map_error! {
    sqlx::Error => Internal,
    std::io::Error => Internal,
    serde_json::Error => BadRequest,
}
```

This generates `impl From<T> for HttpError` for each entry, calling `.to_string()` on the source error.

## Custom error types with `#[derive(ApiError)]`

For production applications, use `#[derive(ApiError)]` to generate `Display`, `IntoResponse`, and `std::error::Error` automatically:

```rust
use r2e::prelude::*; // ApiError, HttpError

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
}
```

Then use it in handlers:

```rust
#[get("/{id}")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, MyError> {
    let file = std::fs::read_to_string("data.json")?; // ? converts io::Error → MyError::Io

    let user = self.service.find(id).await
        .ok_or_else(|| MyError::NotFound(format!("{id}")))?;

    Ok(Json(user))
}
```

All variants produce a JSON response `{"error": "<message>"}` with the appropriate status code.

### Variant attribute: `#[error(...)]`

Every variant **must** have an `#[error(...)]` attribute:

| Form | Effect |
|------|--------|
| `#[error(status = NOT_FOUND, message = "...")]` | Explicit status + message |
| `#[error(status = BAD_REQUEST)]` | Status only, message is inferred (see below) |
| `#[error(status = 429, message = "...")]` | Numeric status code |
| `#[error(transparent)]` | Delegates `Display` + `IntoResponse` to the inner type |
| `#[error(rejection)]` | The variant holds a single `Rejection`; makes the enum an [error envelope](#rejection-envelopes-and-projection) (`From<Rejection>` + `ErrorSchema` are generated) |

### Message interpolation

Messages support `format!`-style placeholders:

```rust
// Tuple fields: {0}, {1}, ...
#[error(status = NOT_FOUND, message = "Resource {0} not found")]
NotFound(String),

// Named fields: {field_name}
#[error(status = BAD_REQUEST, message = "Field {field} is invalid: {reason}")]
InvalidField { field: String, reason: String },
```

### Message inference (when `message` is omitted)

| Variant kind | Inferred message |
|-------------|------------------|
| Single `String` field | Uses the field value |
| `#[from]` field | `source.to_string()` |
| Unit variant | Humanized name (`AlreadyExists` → `"Already exists"`) |

### `#[from]` — automatic `From` conversion

```rust
#[derive(Debug, ApiError)]
pub enum MyError {
    #[error(status = INTERNAL_SERVER_ERROR, message = "IO error")]
    Io(#[from] std::io::Error),
}

// Now you can use: let err: MyError = io_error.into();
// std::error::Error::source(&err) returns the inner io::Error
```

When `message` is omitted on a `#[from]` variant, the source error's `.to_string()` is used.

### `#[error(transparent)]` — delegation

Delegates both `Display` and `IntoResponse` to the inner type:

```rust
#[derive(Debug, ApiError)]
pub enum AppError {
    #[error(transparent)]
    Http(#[from] HttpError),
}

// AppError::Http(HttpError::Forbidden("no access".into()))
// → status 403, body {"error": "no access"}
```

`ApiError` can only be derived on enums.

### Generated traits

`#[derive(ApiError)]` generates:

- `impl Display` — formats the error message
- `impl IntoHttpResponse` (+ the bridging `IntoResponse` impl) — converts to an HTTP response with JSON body
- `impl std::error::Error` — `source()` returns the inner `#[from]` error if present
- `impl From<T>` — one per `#[from]` variant
- `impl From<Rejection>` + `impl ErrorSchema` — when one variant is
  `#[error(rejection)]` (or `#[error(transparent)]` over a `Rejection` field).
  `status_of` is the default status table, `body_schema` delegates to
  `HttpError`'s, `extra_statuses` lists the other variants' fixed statuses and
  `opaque_passthrough()` is `true`. An enum whose only link to the framework is
  `#[error(transparent)] Http(#[from] HttpError)` inherits both impls through
  `HttpError`.

## Rejection, envelopes and projection

Everything the framework rejects before or around your handler — a malformed
JSON body, a missing `content-type`, a bad path segment, a failed JWT, a denied
guard, a garde report, a rate limit, a failed managed `acquire` — is one typed
value: `Rejection` (in the prelude). A `Rejection` is **projected once per
route** into an *error envelope* `E` and only then rendered, so a route has
exactly one wire shape for its errors, whoever raised them. Layers connect
through plain `From`/`Into`; one small trait, `ErrorSchema`, carries the static
side the OpenAPI builder needs.

### The `Rejection` hub

```rust
let kind: RejectionKind = r.kind;   // closed enum, `#[non_exhaustive]`
let status: StatusCode = r.status;  // starts at `kind.default_status()`
let message: &str = &r.message;     // client-facing; `Display` prints it
let details = r.details.as_ref();   // Option<serde_json::Value> — garde field errors
let headers = &r.headers;           // `WWW-Authenticate`, `Retry-After`, …
```

`RejectionKind::default_status()` is the single status table:

| Kind | Status |
|------|--------|
| `MissingContentType`, `UnsupportedMediaType` | 415 |
| `PayloadTooLarge` | 413 |
| `BodyRead`, `MalformedBody`, `InvalidPath`, `InvalidQuery`, `InvalidForm`, `InvalidHeader`, `BadRequest`, `Validation` | 400 |
| `InvalidBody` (deserialized but semantically wrong) | 422 |
| `Unauthenticated` | 401 |
| `Forbidden` | 403 |
| `NotFound` | 404 |
| `MethodNotAllowed` | 405 |
| `Conflict` | 409 |
| `RateLimited` | 429 |
| `Internal` | 500 |
| `Unavailable` | 503 |
| `Timeout` | 504 |
| `Opaque` | status of a pre-rendered `Response` the framework could not type |

Faults reach the hub through std `From`, implemented in the crate that owns the
fault: the axum `Json`/`Path`/`Query`/`Form` rejections, `ParamError`,
`HttpError`, `GuardError`, `SecurityError` (401 + `WWW-Authenticate: Bearer`, or
503 for a JWKS failure), `RolesDenied`, `RateLimited` (sets `Retry-After`),
`FgaDenied`, `TenantError`, a garde `Report`, and any `Response` (→ `Opaque`).
Build one yourself with `Rejection::new(kind, msg)`,
`Rejection::with_status(kind, status, msg)`, `Rejection::from_status(status, msg)`
or the shortcuts `unauthenticated()` / `forbidden(msg)` / `not_found(msg)` /
`bad_request(msg)` / `internal(msg)`, then chain `.details(value)`,
`.header(name, value)` or `.source(err)`.

### Error envelopes

An **envelope** is the type a `Rejection` is projected into before rendering.
`HttpError` is the default and reproduces the plain bodies (`{"error": msg}`,
`{"error":"Validation failed","details":[…]}`). Any other wire shape — an
OpenAI-style `{"error":{"type":…}}`, RFC 9457 problem details, … — is a type
`E` with three impls:

- `From<Rejection>` — build the body from `r.kind` / `r.message` / `r.details`
  and **read `r.status`**, never the table: the status may have been remapped,
  or carried by the fault (a configured tenant status, a 413 body read).
- `IntoHttpResponse` (+ `impl_into_response!`), as for any response type.
- `ErrorSchema` — the static side, read by the runtime **and** the OpenAPI
  builder, so the spec cannot say 422 where the server answers 400:

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
`opaque_passthrough`, calls `E::from(rejection).into_http_response()` and merges
the hub headers into the response (the envelope's own header of the same name
wins).

The quickest envelope is a derive — mark **one** variant `#[error(rejection)]`
over a single `Rejection` field:

```rust
#[derive(Debug, ApiError)]
pub enum ApiEnvelope {
    #[error(status = CONFLICT, message = "already exists: {0}")]
    Duplicate(String),

    #[error(rejection)]          // From<Rejection> + ErrorSchema come with it
    Rejected(Rejection),
}
```

The variant takes no `status`/`message`; to remap statuses, implement
`ErrorSchema` by hand instead of deriving.

### Which envelope renders a route

There is no attribute to wire an envelope: the **handler's return type** decides.

- `Result<T, E>` with `E: From<Rejection> + IntoHttpResponse + ErrorSchema` —
  every framework failure on that route (extractor rejection, failed identity,
  guard denial, garde report, managed acquire/finalize) is projected through
  `E`, so the handler's own `Err(E)` and the framework's errors share one wire
  shape. `HttpError` qualifies, so a `Result<T, HttpError>` route renders
  `HttpError` bodies regardless of the app-level projection. `T` is never
  constrained: `Result<impl IntoResponse, E>` keeps `E` too.
- Any other return type (`Json<T>`, a bare `impl IntoResponse`, a `Result<T, E>`
  whose `E` lacks one of the three traits, a plain `String`) — the route uses
  the **app-level** projection described below.

A route declared infallible (`-> Json<T>`) is a definition the framework
trusts: its failures still exist (a bad body, a missing identity) and render
with the app-level envelope. Declare `Result<T, E>` when the route must speak
`E`.

### App-level projection: `error_projection::<E>()`

```rust
let state = AppBuilder::new()
    .error_projection::<ApiEnvelope>()   // default: HttpError
    .build_state()
    .await;
```

This is the JAX-RS / Quarkus `ExceptionMapper` equivalent: one place that
decides how unmapped failures look. It provides an `ErrorProjector` bean
(`project(rejection) -> Response`) that plugins and hand-written handlers can
inject to render a `Rejection` with the app's envelope. The framework's own
responses go through it too:

| Situation | Kind | Default body |
|-----------|------|--------------|
| Panicking handler | `Internal` (500) | `{"error":"Internal server error"}` |
| Unknown route (unless the app installed its own fallback: a controller `#[fallback]`, a merged `Router::fallback`, the static SPA fallback) | `NotFound` (404) | `{"error":"Not found"}` |
| Known path, wrong method (`Allow` header kept) | `MethodNotAllowed` (405) | `{"error":"Method not allowed"}` |
| Oversized body | `PayloadTooLarge` (413) | `{"error": <body-limit message>}` |

`E::status_of` applies to them like to any other kind.

### Runtime order

The order on every route is fixed:

```text
pre-auth guards → identity + #[inject(request)] fields → guards
→ path/query/header parameters → body → garde validation
→ managed acquire → interceptors → handler → managed finalize
```

A failure at any step is projected once, through the route's envelope, and the
body is never read before identity and guards have passed.

### Worked example: RFC 9457 problem details

The demo app ships a complete custom envelope. `Problem`
(`examples/example-app/src/error.rs`) renders `application/problem+json`, maps
`Validation` to 422 and keeps the `RejectionKind` as a machine-readable `code`:

```rust
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<serde_json::Value>,
}

impl From<Rejection> for Problem {
    fn from(r: Rejection) -> Self {
        let mut p = Problem::new(r.status, r.message.into_owned()); // read r.status
        p.code = Some(format!("{:?}", r.kind));
        p.errors = r.details;
        p
    }
}

impl ErrorSchema for Problem {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            other => other.default_status(),
        }
    }
    fn body_schema() -> Option<(String, serde_json::Value)> {
        Some(("Problem".into(), serde_json::to_value(schemars::schema_for!(Problem)).ok()?))
    }
}
```

`ProblemController` (`examples/example-app/src/controllers/problem_controller.rs`)
exposes three routes side by side: `POST /problems/` returns
`Result<Json<Ticket>, Problem>`, so a missing token, a malformed body or a garde
report all answer as a `Problem`; `GET /problems/{id}` returns the same envelope
from the handler itself; `GET /problems/legacy/{id}` returns a bare `Json<..>`
and therefore renders a bad `id` through the app-level envelope installed in
`app.rs` (`.error_projection::<AppError>()`). The integration tests in
`examples/example-app/tests/http/error_envelope.rs` pin every one of these
bodies and the OpenAPI responses they produce.

## Manual custom error types

You can also implement R2E's `IntoHttpResponse` manually without the derive
macro, then emit the HTTP-backend bridge with `impl_into_response!`:

```rust
use r2e::prelude::*; // IntoHttpResponse, IntoResponse, Response, StatusCode, Json

#[derive(Debug)]
pub enum MyHttpError {
    NotFound(String),
    Database(String),
}

impl IntoHttpResponse for MyHttpError {
    fn into_http_response(self) -> Response {
        let (status, message) = match self {
            MyHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MyHttpError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// Bridges to the HTTP backend's response contract — this is what makes
// `Result<T, MyHttpError>` returnable from a handler. Non-generic types only.
r2e::http::impl_into_response!(MyHttpError);
```

`IntoHttpResponse` is R2E's own trait, so an error type written this way does
not name the HTTP backend. Implementing axum's `IntoResponse` directly still
compiles, but it couples the type to that backend.

## Panic catching

Panic capture is always on — no plugin, no opt-in. A panicking handler answers
a 500 (`Rejection::internal("Internal server error")` rendered through the
[app-level envelope](#app-level-projection-error_projectione), so
`{"error":"Internal server error"}` by default) and R2E emits **one** structured `error`
event on target `r2e::panic` (`panic_message` + `route`) from inside the
request span, so it carries the request's `request_id`; the request still gets
its `request completed` summary line and its 5xx metric series. Backtraces are
left to the `std` panic hook.

The same containment covers background work: a panicking `#[scheduled]` tick or
`PoolExecutor` job (`#[async_exec]`, `executor.submit`) is caught in the pool,
the job is marked failed (`JoinError::is_panic()`), the next tick is still
scheduled, and one `r2e::panic` error line is emitted with `task = <name>` or
`job = <name>` instead of `route`.

R2E increments no metric of its own. To count panics in your own registry,
register a hook — it fires once per panic, whatever the origin:

```rust
AppBuilder::new()
    .on_panic(move |report| {
        // report.message() -> &str
        // report.origin() -> PanicOrigin<'_>:
        //   Http { route: Option<&str> } | Scheduled { task: &str } | Executor { job: Option<&str> }
        // report.label() -> &str — bounded label for any origin: route template
        //   (or the metrics' `unmatched`), task name, or job name (`<unnamed>`)
        // report.route() / report.route_label() -> HTTP-oriented accessors
        panics.with_label_values(&[report.label()]).inc();
    })
    // ...
```

Keep the hook short and non-blocking: it runs on the panicking task while the
panic is being converted to its outcome. It cannot break that outcome — a
panic inside the hook is caught and logged, and the JSON 500 (or the failed
job) is unchanged.

## Error wrappers for managed resources

The `ManagedResource` trait requires `Error: Into<Rejection>`, so an `acquire`
or `finalize` failure is projected through the route's error envelope exactly
like an extractor rejection. Due to Rust's orphan rules, you can't implement
`Into<Rejection>` for a type you don't own. R2E provides `ManagedErr<E>` — a
wrapper over any `E: Into<Rejection>`. For the framework's built-in `HttpError`,
use `ManagedErr<HttpError>`.

```rust
impl<S: BeanLookup + Send + Sync> ManagedResource<S> for Tx<'static, Sqlite> {
    type Error = ManagedErr<MyHttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        // Resolve the pool from the bean graph by type (no `HasPool` trait).
        let pool = context.state.bean::<Pool<Sqlite>>()
            .ok_or_else(|| ManagedErr(MyHttpError::Database("pool bean not found".into())))?;
        let tx = pool.begin().await
            .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
        Ok(Tx(tx))
    }
    // ...
}
```

See [Managed Resources](../advanced/managed-resources.md) for details.
