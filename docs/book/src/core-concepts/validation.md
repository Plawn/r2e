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
| `#[param(default)]` | — | Uses `Default::default()` when absent |
| `#[param(default = expr)]` | — | Uses `expr` when absent |

- `Option<T>` fields are optional — absent values become `None`
- Non-Option fields are required — absent values return 400 Bad Request
- `#[param(default)]` uses `Default::default()` when the parameter is absent
- `#[param(default = expr)]` uses the given expression when absent
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

### Default values

Use `#[param(default)]` or `#[param(default = expr)]` on non-Option fields to provide a fallback when the parameter is absent, instead of returning 400:

```rust
#[derive(Params)]
pub struct PaginationParams {
    #[query]
    #[param(default = 1)]
    pub page: u32,

    #[query]
    #[param(default = 20)]
    pub size: u32,

    #[query]
    #[param(default)]           // uses Default::default() → ""
    pub sort: String,
}
```

- `#[param(default)]` — calls `Default::default()` (the field type must implement `Default`)
- `#[param(default = expr)]` — uses the given expression (e.g., `1`, `"name".to_string()`, `MyEnum::Asc`)

### OpenAPI integration

`#[derive(Params)]` also generates an implementation of `ParamsMetadata`, which feeds parameter metadata (name, location, type, required) into the OpenAPI spec. When a `Params` struct is used as a handler parameter, its fields automatically appear in the generated `/openapi.json` — no manual annotation needed.

### Params without validation

`#[derive(Params)]` works on its own without `Validate`. In that case only extraction and type parsing are performed:

```rust
#[derive(Params)]
pub struct PaginationParams {
    #[query]
    #[param(default = 1)]
    pub page: u32,

    #[query]
    #[param(default = 20)]
    pub size: u32,
}
```

## How it works

The `#[routes]` macro generates validation calls using an autoref specialization trick:

1. Deserialization via `Json<T>` / extraction via `Params` (standard Axum `FromRequest` / `FromRequestParts`)
2. The generated handler code calls `(&__AutoValidator(&value)).__maybe_validate()`
3. Method resolution picks the right implementation at compile time:
   - If `T: garde::Validate` → `__DoValidate` trait (direct match, higher priority) → runs validation
   - Otherwise → `__SkipValidate` trait (autoref fallback, lower priority) → no-op, zero overhead
4. On validation failure, `garde::Report` is converted directly to a 400 Bad Request response (bypassing `HttpError`):
   - Each field error has `field` (path like `"email"` or `"users[0].name"`), `message`, and `code` (`"validation"`)
   - Empty paths (top-level errors) become `"value"`
5. The response is returned before the handler body runs — an invalid request never reaches your code

The JSON response structure is identical to `HttpError::Validation` (see [Error Handling](./error-handling.md)), but the validation path produces the response directly without going through `HttpError`.
