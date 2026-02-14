# Validation

R2E provides a `Validated<T>` extractor that validates request bodies using the `validator` crate.

## Setup

Enable the `validation` feature:

```toml
[dependencies]
r2e = { version = "0.1", features = ["validation"] }
validator = { version = "0.18", features = ["derive"] }
```

## Defining validation rules

Derive `Validate` on your request types:

```rust
use serde::Deserialize;
use validator::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(email)]
    pub email: String,

    #[validate(range(min = 0, max = 150))]
    pub age: Option<i32>,

    #[validate(url)]
    pub website: Option<String>,
}
```

### Available rules

| Rule | Description |
|------|-------------|
| `length(min = N, max = N)` | String length constraints |
| `email` | Valid email format |
| `url` | Valid URL format |
| `range(min = N, max = N)` | Numeric range |
| `regex(path = "PATTERN")` | Regex match |
| `contains(pattern = "str")` | String contains substring |
| `must_match(other = "field")` | Two fields must be equal |
| `custom(function = "fn_name")` | Custom validation function |

## Using `Validated<T>`

Replace `Json<T>` with `Validated<T>` in your handler:

```rust
use r2e::prelude::*;

#[post("/")]
async fn create(&self, Validated(body): Validated<CreateUserRequest>) -> Json<User> {
    Json(self.user_service.create(body.name, body.email).await)
}
```

`Validated<T>` performs two steps:
1. Deserializes the JSON body (returns 400 on parse error)
2. Runs validation rules (returns 400 with field-level errors)

## Error response format

On validation failure, R2E returns a 400 response with structured errors:

```json
{
  "error": "Validation failed",
  "fields": {
    "name": [
      {
        "code": "length",
        "message": null,
        "params": {
          "min": 1,
          "max": 100,
          "value": ""
        }
      }
    ],
    "email": [
      {
        "code": "email",
        "message": null,
        "params": {
          "value": "not-an-email"
        }
      }
    ]
  }
}
```

## JSON deserialization errors

If the body can't be deserialized (e.g., wrong types, missing required fields), R2E returns a 400 before validation runs:

```json
{
  "error": "Invalid JSON: missing field `name` at line 1 column 2"
}
```

## Custom validation functions

```rust
fn validate_username(username: &str) -> Result<(), validator::ValidationError> {
    if username.contains(' ') {
        return Err(validator::ValidationError::new("no_spaces"));
    }
    Ok(())
}

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[validate(custom(function = "validate_username"))]
    pub username: String,
}
```
