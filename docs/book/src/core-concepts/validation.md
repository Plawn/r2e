# Validation

R2E validates request bodies automatically using the `garde` crate. If a type derives `garde::Validate`, validation runs transparently when the body is extracted — no special wrapper needed.

## Setup

```toml
[dependencies]
r2e = { version = "0.1", features = ["full"] }
garde = { version = "0.22", features = ["derive", "email"] }
```

Validation is always available — no feature flag required.

## Defining validation rules

Derive `Validate` on your request types:

```rust
use serde::Deserialize;
use garde::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,

    #[garde(email)]
    pub email: String,

    #[garde(range(min = 0, max = 150))]
    pub age: Option<i32>,

    #[garde(url)]
    pub website: Option<String>,
}
```

### Available rules (garde)

| Rule | Attribute | Description |
|------|-----------|-------------|
| Length | `#[garde(length(min = 1, max = 100))]` | String length constraints |
| Email | `#[garde(email)]` | Valid email format |
| URL | `#[garde(url)]` | Valid URL format |
| Range | `#[garde(range(min = 0, max = 1000))]` | Numeric range |
| Pattern | `#[garde(pattern("regex"))]` | Regex match |
| Custom | `#[garde(custom(my_fn))]` | Custom validation function |
| Skip | `#[garde(skip)]` | Don't validate this field |

## Using in handlers

Use `Json<T>` normally — validation is automatic:

```rust
use r2e::prelude::*;

#[post("/")]
async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
    // `body` is guaranteed valid here
    Json(self.user_service.create(body.name, body.email).await)
}
```

Types without `#[derive(Validate)]` work normally — no validation is performed (zero overhead).

## Error response format

On validation failure, R2E returns a 400 response with structured errors:

```json
{
    "error": "Validation failed",
    "details": [
        {
            "field": "email",
            "message": "not a valid email address",
            "code": "validation"
        },
        {
            "field": "name",
            "message": "length is lower than 1",
            "code": "validation"
        }
    ]
}
```

## JSON deserialization errors

If the body can't be deserialized (e.g., wrong types, missing required fields), R2E returns a 400 before validation runs:

```json
{
    "error": "Failed to deserialize the JSON body ..."
}
```

## Custom validation functions

```rust
fn validate_username(value: &str, _ctx: &()) -> garde::Result {
    if value.contains(' ') {
        return Err(garde::Error::new("username must not contain spaces"));
    }
    Ok(())
}

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[garde(custom(validate_username))]
    pub username: String,
}
```

## Params — aggregated parameter extraction

`#[derive(Params)]` lets you group path, query, and header parameters into a single struct, similar to JAX-RS `@BeanParam`. Combined with `garde::Validate`, all parameters are extracted **and** validated in one step.

### Defining a Params struct

```rust
use r2e::prelude::*;
use garde::Validate;

#[derive(Params, Validate)]
pub struct GetUserParams {
    #[path]
    #[garde(skip)]
    pub id: u64,

    #[query]
    #[garde(range(min = 1))]
    pub page: Option<u32>,

    #[query(name = "q")]
    #[garde(skip)]
    pub search: Option<String>,

    #[header("X-Tenant-Id")]
    #[garde(length(min = 1))]
    pub tenant_id: String,
}
```

### Field attributes

| Attribute | Source | Default name |
|-----------|--------|-------------|
| `#[path]` | URL path segments | Field name |
| `#[path(name = "userId")]` | URL path segments | Custom name |
| `#[query]` | Query string | Field name |
| `#[query(name = "q")]` | Query string | Custom name |
| `#[header("X-Custom")]` | HTTP headers | Explicit (required) |

- `Option<T>` fields are optional — absent values become `None`
- Non-Option fields are required — absent values return 400 Bad Request
- Values are parsed via `FromStr` (supports `u32`, `u64`, `i64`, `String`, `bool`, `Uuid`, etc.)

### Using in handlers

```rust
#[routes]
impl UserController {
    #[get("/{id}")]
    async fn get_user(&self, params: GetUserParams) -> Json<User> {
        // params.id, params.page, params.search, params.tenant_id
        // are all extracted and validated automatically
        let user = self.user_service.find(params.id).await;
        Json(user)
    }
}
```

### Error responses

Missing or unparseable parameters return 400:

```json
{
    "error": "Missing path parameter 'id'"
}
```

```json
{
    "error": "Invalid query parameter 'page': parse error"
}
```

If the struct also derives `Validate`, validation errors are returned in the same format as JSON body validation.

### Params without validation

`#[derive(Params)]` works on its own without `Validate`. In that case only extraction and type parsing are performed:

```rust
#[derive(Params)]
pub struct PaginationParams {
    #[query]
    pub page: Option<u32>,

    #[query]
    pub size: Option<u32>,
}
```

## How it works

The `#[routes]` macro generates validation calls using an autoref specialization trick:

1. Deserialization via `Json<T>` / extraction via `Params` (standard Axum `FromRequest` / `FromRequestParts`)
2. Automatic validation via `__AutoValidator` — if the type derives `Validate`, validation runs; otherwise it's a no-op (zero overhead)
3. On failure, returns 400 with per-field error details
