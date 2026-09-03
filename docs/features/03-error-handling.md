# Feature 3 — Error Handling

## TL;DR

Structured errors that convert automatically to consistent JSON HTTP responses. Return `Result<T, HttpError>` from handlers; each `HttpError` variant maps to a status (`NotFound`→404, `Unauthorized`→401, `Forbidden`→403, `BadRequest`→400, `Internal`→500, `Custom { status, body }`→anything). Generate `From<E> for HttpError` conversions in one line with `map_error!`. A Tower layer captures handler panics and turns them into 500s.


## Goal

Provide a structured error system that automatically converts errors into consistent JSON HTTP responses, with support for custom errors and panic capture.

## Key concepts

### HttpError

`HttpError` is the central enum representing all application errors. Each variant corresponds to a specific HTTP status code.

### map_error!

Macro to generate `From<E> for HttpError` implementations in a single line.

### Catch panic

Tower layer that captures panics in handlers and converts them into 500 responses.

## HttpError variants

The enum is `#[non_exhaustive]`; the message variants carry `Cow<'static, str>`
(so both `String` and `&'static str` work with `.into()`).

| Variant | HTTP Code | Usage |
|---------|-----------|-------|
| `NotFound(Cow<'static, str>)` | 404 | Resource not found |
| `Unauthorized(Cow<'static, str>)` | 401 | Authentication required/invalid |
| `Forbidden(Cow<'static, str>)` | 403 | Insufficient permissions |
| `BadRequest(Cow<'static, str>)` | 400 | Malformed request |
| `Internal(Cow<'static, str>)` | 500 | Server error |
| `Validation(ValidationErrorResponse)` | 400 | Validation failure (feature `validation`) |
| `Custom { status, body }` | Custom | Arbitrary HTTP code and JSON body |
| `WithSource { status, message, source }` | Custom | Preserves the source error chain (produced by `From` conversions); only `message` is sent to the client |

## Usage

### 1. Returning standard errors

```rust
#[get("/users/{id}")]
async fn get_by_id(
    &self,
    Path(id): Path<u64>,
) -> Result<axum::Json<User>, r2e_core::HttpError> {
    match self.user_service.get_by_id(id).await {
        Some(user) => Ok(axum::Json(user)),
        None => Err(r2e_core::HttpError::NotFound("User not found".into())),
    }
}
```

Generated response:

```http
HTTP/1.1 404 Not Found
Content-Type: application/json

{"error": "User not found"}
```

### 2. Custom errors with arbitrary HTTP code

The `Custom` variant allows returning any HTTP code with a free-form JSON body:

```rust
#[get("/error/custom")]
async fn custom_error(&self) -> Result<axum::Json<()>, r2e_core::HttpError> {
    Err(r2e_core::HttpError::Custom {
        status: axum::http::StatusCode::from_u16(418).unwrap(),
        body: serde_json::json!({
            "error": "I'm a teapot",
            "code": 418
        }),
    })
}
```

Response:

```http
HTTP/1.1 418 I'm a Teapot
Content-Type: application/json

{"error": "I'm a teapot", "code": 418}
```

### 3. Automatic conversions with `From`

`HttpError` implements `From<std::io::Error>` out of the box, enabling `?`:

```rust
// Included by default
impl From<std::io::Error> for HttpError { ... }
```

For other error types (including `sqlx::Error`), generate the conversion yourself
with `map_error!` (see below) or convert at the call site with
`.map_err(|e| HttpError::internal(e.to_string()))`. R2E core does not ship a
built-in `From<sqlx::Error>` impl.

### 4. The `map_error!` macro

To add additional conversions in your application code:

```rust
r2e_core::map_error! {
    serde_json::Error => Internal,
    reqwest::Error => Internal,
}
```

This generates:

```rust
impl From<serde_json::Error> for HttpError {
    fn from(err: serde_json::Error) -> Self {
        HttpError::Internal(err.to_string())
    }
}
```

**Note**: `map_error!` generates `impl From` — both types (source error and `HttpError`) must respect the coherence rule (orphan rule). Use it only for error types defined in your crate, or in the crate where `HttpError` is defined.

### 5. Catch panic (Tower layer)

Panic capture is **always on** — no plugin, no opt-in. If a handler panics,
instead of a crash the client receives:

```http
HTTP/1.1 500 Internal Server Error
Content-Type: application/json

{"error": "Internal server error"}
```

The layer sits *below* the tracing and metrics layers, so the panicking
request is still instrumented like any other 500: the `request completed`
summary line, the 5xx metric series and the `x-request-id` echo all happen.
On top of that the layer emits one structured `error` event on target
`r2e::panic`, inside the request span — so it carries the request's
`request_id` and `route`:

```
ERROR r2e::panic: handler panicked; responding 500
      panic_message="boom" route="/users/{id}"
      (span) request_id="c0ffee…" route="/users/{id}"
```

The panic payload is downcast from `&'static str` and `String`; anything else
(a `panic_any` with a custom type) logs `<non-string panic payload>`. The
backtrace is deliberately left to the `std` panic hook — capturing one per
panic inside the layer is expensive and rarely what a service wants.

#### Counting panics: `on_panic`

R2E increments no metric of its own here — every service owns its registry
and its metric prefix. Register a hook instead:

```rust
let panics = panic_counter.clone(); // your own metric
AppBuilder::new()
    .on_panic(move |report| {
        panics.with_label_values(&[report.route_label()]).inc();
    })
    // ...
```

The hook receives a `PanicReport` (in the prelude) — `message()`, and the
bounded route template as `route() -> Option<&str>` or `route_label() -> &str`
(the template, or the same `unmatched` label the HTTP metrics use, so a panic
counter lines up with the RED series). Nothing request-borne: no body, no
headers, no path parameters. It runs on the request task while the panic
is being converted to a response, so keep it short and non-blocking. It cannot
break the response: a panic inside the hook is caught and logged once, and the
JSON 500 still goes out.

The `ErrorHandling` plugin still exists and adds one more catch-panic layer
at its own install slot, but it is no longer needed for panic capture.

## Combination with other features

### With Validation (#2)

When the `validation` feature flag is active, `HttpError::Validation` provides a structured 400 response with per-field details:

```json
{
    "error": "Validation failed",
    "details": [
        {"field": "email", "message": "...", "code": "email"}
    ]
}
```

### With `#[managed]` transactions (#4)

Error responses (`4xx`/`5xx`) roll the managed transaction back automatically:

```rust
#[post("/users/db")]
async fn create_in_db(&self, #[managed] tx: &mut Tx<'_, Sqlite>) -> Result<Json<User>, HttpError> {
    // If this returns an error response, the transaction is rolled back
    sqlx::query("INSERT INTO users ...").execute(tx.as_mut()).await?;
    Ok(...)
}
```

## Validation criteria

```bash
# 404 error
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/999
# → {"error":"User not found"}

# Custom 418 error
curl -H "Authorization: Bearer <token>" http://localhost:3000/error/custom
# → {"error":"I'm a teapot","code":418}
```
