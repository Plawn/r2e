# Error Handling

R2E provides a built-in `AppError` type and supports custom error types that integrate with Axum's response system.

## Built-in `AppError`

`AppError` maps common error cases to HTTP status codes:

```rust
use r2e_core::AppError;

#[get("/{id}")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, AppError> {
    self.service.get_by_id(id).await
        .map(Json)
        .ok_or_else(|| AppError::NotFound("User not found".into()))
}
```

### Variants

| Variant | HTTP Status | JSON body |
|---------|------------|-----------|
| `AppError::NotFound(msg)` | 404 | `{"error": "User not found"}` |
| `AppError::Unauthorized(msg)` | 401 | `{"error": "..."}` |
| `AppError::Forbidden(msg)` | 403 | `{"error": "..."}` |
| `AppError::BadRequest(msg)` | 400 | `{"error": "..."}` |
| `AppError::Internal(msg)` | 500 | `{"error": "..."}` |
| `AppError::Custom { status, body }` | any | custom JSON body |

### Custom status codes

```rust
#[post("/")]
async fn create(&self, body: Json<Request>) -> Result<Json<Response>, AppError> {
    Err(AppError::Custom {
        status: StatusCode::CONFLICT,
        body: serde_json::json!({
            "error": "duplicate_entry",
            "message": "A user with this email already exists",
        }),
    })
}
```

## Custom error types

For production applications, define your own error type:

```rust
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use axum::Json;

#[derive(Debug)]
pub enum MyAppError {
    NotFound(String),
    Database(String),
    Validation(String),
    Internal(String),
}

impl IntoResponse for MyAppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            MyAppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MyAppError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            MyAppError::Validation(msg) => (StatusCode::BAD_REQUEST, msg),
            MyAppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// Automatic conversion from external errors
impl From<sqlx::Error> for MyAppError {
    fn from(err: sqlx::Error) -> Self {
        MyAppError::Database(err.to_string())
    }
}
```

Then use it in handlers:

```rust
#[get("/{id}")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, MyAppError> {
    let user = sqlx::query_as!(User, "SELECT * FROM users WHERE id = ?", id)
        .fetch_optional(&self.pool)
        .await?;  // ? converts sqlx::Error → MyAppError::Database

    user.map(Json)
        .ok_or_else(|| MyAppError::NotFound(format!("User {} not found", id)))
}
```

## Panic catching

Install the `ErrorHandling` plugin to catch panics and return JSON 500 responses instead of crashing:

```rust
AppBuilder::new()
    .build_state::<AppState, _>()
    .await
    .with(ErrorHandling)
    // ...
```

Without this, a panic in a handler will drop the connection with no response.

## Error wrappers for managed resources

The `ManagedResource` trait requires `Error: Into<Response>`. Due to Rust's orphan rules, you can't implement `Into<Response>` directly for your error type. R2E provides wrappers:

- `ManagedError` — wraps the built-in `AppError`
- `ManagedErr<E>` — wraps any error type implementing `IntoResponse`

```rust
impl<S: HasPool + Send + Sync> ManagedResource<S> for Tx<'static, Sqlite> {
    type Error = ManagedErr<MyAppError>;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state.pool().begin().await
            .map_err(|e| ManagedErr(MyAppError::Database(e.to_string())))?;
        Ok(Tx(tx))
    }
    // ...
}
```

See [Managed Resources](../advanced/managed-resources.md) for details.
