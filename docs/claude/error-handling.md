# Error Handling & Managed Resources

## HttpError (r2e-core)

R2E provides `HttpError` as a default error type, `#[derive(ApiError)]` for custom error types, and automatic validation error handling via garde integration. `HttpError` implements `std::error::Error`, `Clone`, `Display`, and `IntoResponse`.

### `HttpError` variants

The enum is `#[non_exhaustive]` — always include a wildcard arm when matching.

| Variant | Status | Body |
|---------|--------|------|
| `NotFound(Cow<'static, str>)` | 404 | `{"error": "..."}` |
| `Unauthorized(Cow<'static, str>)` | 401 | `{"error": "..."}` |
| `Forbidden(Cow<'static, str>)` | 403 | `{"error": "..."}` |
| `BadRequest(Cow<'static, str>)` | 400 | `{"error": "..."}` |
| `Internal(Cow<'static, str>)` | 500 | `{"error": "..."}` |
| `Validation(ValidationErrorResponse)` | 400 | `{"error": "Validation failed", "details": [...]}` |
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
    serde_json::Error => BadRequest,
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

Generates `Display`, `IntoResponse`, and `std::error::Error` impls automatically. Available in the prelude.

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

Alternatively, implement `IntoResponse` manually (match variant → `(StatusCode, Json)` tuple).

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

**Automatic 5xx logging:** The `ErrorHandling` plugin (applied via `.with(ErrorHandling)`) wraps the router with `CatchPanicLayer` and returns a 500 JSON response on panics. For custom 5xx logging, add a middleware layer that inspects response status codes.

**Key files:** `r2e-core/src/error.rs` (HttpError, `error_response()`, `map_error!`, `HttpErrorExt`), `r2e-macros/src/api_error_derive.rs` (derive implementation), `r2e-core/tests/api_error.rs` (comprehensive tests)

---

## Guards (error helpers)

The `GuardError` struct simplifies guard error construction:

```rust
use r2e_core::guards::GuardError;

// Instead of manually building Response:
Err(GuardError::forbidden("Insufficient permissions").into())
Err(GuardError::unauthorized("Missing API key").into())
Err(GuardError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited").into())
```

---

## Managed Resources (r2e-core)

The `#[managed]` attribute enables automatic lifecycle management for resources like database transactions, connections, scoped caches, or audit contexts. Resources are acquired before handler execution and released after, with success/failure status.

### Core trait

```rust
pub trait ManagedResource<S>: Sized {
    type Error: Into<Response>;

    async fn acquire(state: &S) -> Result<Self, Self::Error>;
    async fn release(self, success: bool) -> Result<(), Self::Error>;
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
        #[managed] tx: &mut Tx<'_, Sqlite>,  // Acquired before, released after
    ) -> Result<Json<User>, MyHttpError> {
        sqlx::query("INSERT INTO users ...").execute(tx.as_mut()).await?;
        Ok(Json(user))
    }
}
```

### Lifecycle

1. `acquire(&state)` — called before handler, resource obtained from app state
2. Handler receives `&mut Resource`
3. `release(self, success)` — called after handler
   - `success = true` if handler returned `Ok` or non-Result type
   - `success = false` if handler returned `Err`

**Transaction wrapper pattern:** Define `Tx<'a, DB>` + `HasPool<DB>` trait, implement `ManagedResource` with `acquire` (begins tx) and `release` (commits on success, drops=rollback on failure). Use `ManagedErr<E>` as error type.

**Note:** `#[managed]` and `#[transactional]` are mutually exclusive. Prefer `#[managed]` for new code.

### Error wrappers for `ManagedResource`

`ManagedErr<E>` — generic wrapper for any `IntoResponse` type. Needed because orphan rules prevent `impl Into<Response> for YourError` directly. Use `ManagedErr<HttpError>` for the common case.

**Note:** `ManagedError` (non-generic) is deprecated in favor of `ManagedErr<HttpError>`.
