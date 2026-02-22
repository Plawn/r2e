# Error Handling

R2E provides a built-in `HttpError` type and supports custom error types that integrate with Axum's response system.

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

## Custom error types

For production applications, define your own error type:

```rust
use r2e::prelude::*; // IntoResponse, Response, StatusCode, Json

#[derive(Debug)]
pub enum MyHttpError {
    NotFound(String),
    Database(String),
    Validation(String),
    Internal(String),
}

impl IntoResponse for MyHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            MyHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MyHttpError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            MyHttpError::Validation(msg) => (StatusCode::BAD_REQUEST, msg),
            MyHttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// Automatic conversion from external errors
impl From<sqlx::Error> for MyHttpError {
    fn from(err: sqlx::Error) -> Self {
        MyHttpError::Database(err.to_string())
    }
}
```

Then use it in handlers:

```rust
#[get("/{id}")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, MyHttpError> {
    let user = sqlx::query_as!(User, "SELECT * FROM users WHERE id = ?", id)
        .fetch_optional(&self.pool)
        .await?;  // ? converts sqlx::Error → MyHttpError::Database

    user.map(Json)
        .ok_or_else(|| MyHttpError::NotFound(format!("User {} not found", id)))
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
