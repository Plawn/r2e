# Error Handling & Managed Resources

## Error Handling (r2e-core)

R2E provides `HttpError` as a default error type, `#[derive(ApiError)]` for custom error types, and automatic validation error handling.

### `HttpError` variants

| Variant | Status | Body |
|---------|--------|------|
| `NotFound(String)` | 404 | `{"error": "..."}` |
| `Unauthorized(String)` | 401 | `{"error": "..."}` |
| `Forbidden(String)` | 403 | `{"error": "..."}` |
| `BadRequest(String)` | 400 | `{"error": "..."}` |
| `Internal(String)` | 500 | `{"error": "..."}` |
| `Validation(ValidationErrorResponse)` | 400 | `{"error": "Validation failed", "details": [...]}` |
| `Custom { status, body }` | any | custom JSON body |

### Using the built-in `HttpError`

```rust
use r2e_core::HttpError;

#[get("/{id}")]
async fn get(&self, Path(id): Path<i64>) -> Result<Json<User>, HttpError> {
    let user = self.service.find(id).await
        .ok_or_else(|| HttpError::NotFound("User not found".into()))?;
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
- `#[error(status = 429)]` — numeric status code
- `#[error(transparent)]` — delegates Display + IntoResponse to the inner type
- `#[from]` on a field — generates `From<T>` impl and `Error::source()` returns that field

**Key files:** `r2e-core/src/error.rs` (HttpError, `error_response()`, `map_error!`), `r2e-macros/src/api_error_derive.rs` (derive implementation), `r2e-core/tests/api_error.rs` (comprehensive tests)

### Validation errors (`HttpError::Validation`)

Produced automatically by the `garde` integration. When `Json<T>` is extracted and `T: garde::Validate`, validation runs before the handler body. On failure, a 400 response is returned:
```json
{"error": "Validation failed", "details": [{"field": "email", "message": "not a valid email", "code": "validation"}]}
```
The underlying types: `ValidationErrorResponse { errors: Vec<FieldError> }` and `FieldError { field, message, code }` (in `r2e-core::validation`). The validation uses an autoref specialization trick (`__AutoValidator` / `__DoValidate` / `__SkipValidate`) so types without `Validate` have zero overhead.

### Manual custom error types

Alternatively, implement `IntoResponse` manually (match variant → `(StatusCode, Json)` tuple).

### Error wrappers for `ManagedResource`

`ManagedError` (wraps `HttpError`) and `ManagedErr<E>` (generic wrapper for any `IntoResponse` type). Needed because orphan rules prevent `impl Into<Response> for YourError` directly.

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
