# Error Handling

R2E provides a built-in `HttpError` type, a `#[derive(ApiError)]` macro for custom error types, and automatic validation error handling via `garde`.

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
| `HttpError::Validation(resp)` | 400 | `{"error": "Validation failed", "details": [...]}` |
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

`HttpError::Validation` carries a `ValidationErrorResponse` with per-field error details. This is the variant produced by automatic `garde` validation (see [Validation](./validation.md)), but you can also construct it manually:

```rust
use r2e_core::validation::{ValidationErrorResponse, FieldError};

Err(HttpError::Validation(ValidationErrorResponse {
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
- `impl IntoResponse` — converts to an HTTP response with JSON body
- `impl std::error::Error` — `source()` returns the inner `#[from]` error if present
- `impl From<T>` — one per `#[from]` variant

## Manual custom error types

You can also implement `IntoResponse` manually without the derive macro:

```rust
use r2e::prelude::*; // IntoResponse, Response, StatusCode, Json

#[derive(Debug)]
pub enum MyHttpError {
    NotFound(String),
    Database(String),
}

impl IntoResponse for MyHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            MyHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MyHttpError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}
```

## Panic catching

Install the `ErrorHandling` plugin to catch panics and return JSON 500 responses instead of crashing:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(ErrorHandling)
    // ...
```

Without this, a panic in a handler will drop the connection with no response.

## Error wrappers for managed resources

The `ManagedResource` trait requires `Error: Into<Response>`. Due to Rust's orphan rules, you can't implement `Into<Response>` directly for your error type. R2E provides wrappers:

- `ManagedError` — wraps the built-in `HttpError`
- `ManagedErr<E>` — wraps any error type implementing `IntoResponse`

```rust
impl<S: HasPool + Send + Sync> ManagedResource<S> for Tx<'static, Sqlite> {
    type Error = ManagedErr<MyHttpError>;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state.pool().begin().await
            .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
        Ok(Tx(tx))
    }
    // ...
}
```

See [Managed Resources](../advanced/managed-resources.md) for details.
