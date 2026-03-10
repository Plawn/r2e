# Feature 3 — Error Handling

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

| Variant | HTTP Code | Usage |
|---------|-----------|-------|
| `NotFound(String)` | 404 | Resource not found |
| `Unauthorized(String)` | 401 | Authentication required/invalid |
| `Forbidden(String)` | 403 | Insufficient permissions |
| `BadRequest(String)` | 400 | Malformed request |
| `Internal(String)` | 500 | Server error |
| `Validation(ValidationErrorResponse)` | 400 | Validation failure (feature `validation`) |
| `Custom { status, body }` | Custom | Arbitrary HTTP code and JSON body |

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

`HttpError` implements `From` for common error types, enabling the use of `?`:

```rust
// Included by default
impl From<std::io::Error> for HttpError { ... }

// Included with the "sqlx" feature flag
impl From<sqlx::Error> for HttpError { ... }
```

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

Enable panic capture in the `AppBuilder`:

```rust
AppBuilder::new()
    .with_state(services)
    .with_error_handling()  // Enables catch_panic_layer
    // ...
```

If a handler panics, instead of a crash, the client receives:

```http
HTTP/1.1 500 Internal Server Error
Content-Type: application/json

{"error": "Internal server error"}
```

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

### With `#[transactional]` (#4)

Errors within a transactional block trigger an automatic rollback of the transaction:

```rust
#[post("/users/db")]
#[transactional]
async fn create_in_db(&self, ...) -> Result<Json<User>, HttpError> {
    // If an error occurs here, tx.rollback() is called automatically
    sqlx::query("INSERT INTO users ...").execute(&mut *tx).await?;
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
