# Feature 2 — Validation

## TL;DR

Automatic request-body validation with declarative rules, returning structured 400s on failure. Derive `garde::Validate` on your DTO and take it as `Json<T>` — the generated handler validates transparently before your body runs. Rules are `garde` attributes: `#[garde(length(min=1, max=100))]`, `#[garde(email)]`, `#[garde(range(...))]`, `#[garde(custom(my_fn))]`, etc. No manual checks; enabled by the `validation` feature.


## Goal

Automatically validate JSON request bodies with declarative rules, and return structured 400 responses on failure.

## Key concepts

### Automatic validation

R2E automatically validates handler parameters that derive `garde::Validate`. Simply derive `Validate` on the type and use `Json<T>` — validation is performed transparently in the generated code.

### The garde crate

R2E uses the `garde` crate to declare validation rules on structs. Garde provides compile-time rule verification and a typed context system.

## Usage

### 1. Define a model with validation rules

```rust
use serde::Deserialize;
use garde::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,

    #[garde(email)]
    pub email: String,
}
```

### Available rules (garde crate)

| Rule | Attribute | Example |
|------|----------|---------|
| Length | `#[garde(length(min=1, max=100))]` | Strings |
| Email | `#[garde(email)]` | Email format |
| URL | `#[garde(url)]` | URL format |
| Range | `#[garde(range(min=0, max=1000))]` | Numbers |
| Pattern | `#[garde(pattern("regex"))]` | Custom patterns |
| Custom | `#[garde(custom(my_fn))]` | Arbitrary logic |
| Skip | `#[garde(skip)]` | Do not validate this field |

### 2. Use in a handler

Use `Json<T>` as usual — validation is automatic:

```rust
use r2e::prelude::*;

#[controller(path = "/")]
pub struct UserController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl UserController {
    #[post("/users")]
    async fn create(
        &self,
        Json(body): Json<CreateUserRequest>,
    ) -> Json<User> {
        // `body` is guaranteed to be valid here
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
    }
}
```

### 3. Response on validation error

If validation fails, R2E automatically returns:

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

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

### 4. Response on invalid JSON

If the body is not valid JSON (deserialization error), a standard 400 is returned:

```json
{
    "error": "Failed to deserialize the JSON body ..."
}
```

## Params — aggregated parameter extraction

`#[derive(Params)]` allows grouping path, query, and header into a single struct (equivalent to `@BeanParam` in JAX-RS). Combined with `garde::Validate`, all parameters are extracted **and** validated in a single step.

### Definition

```rust
use r2e::prelude::*;
use garde::Validate;

#[derive(Params, Validate)]
pub struct GetUserParams {
    #[param(path)]
    #[garde(skip)]
    pub id: u64,

    #[query]
    #[garde(range(min = 1))]
    pub page: Option<u32>,

    #[header("X-Tenant-Id")]
    #[garde(length(min = 1))]
    pub tenant_id: String,
}
```

### Available attributes

| Attribute | Source | Default name |
|-----------|--------|-------------|
| `#[param(path)]` | Path segments | Field name |
| `#[param(path, name = "userId")]` | Path segments | Custom name |
| `#[query]` | Query string | Field name |
| `#[query(name = "q")]` | Query string | Custom name |
| `#[header("X-Custom")]` | HTTP headers | Explicit name (required) |

- A field with **no** attribute is a query parameter named after the field
- `Option<T>` → optional parameter (absent = `None`)
- Non-Option `T` → required parameter (absent = 400 Bad Request)
- `#[param(default)]` → uses `Default::default()` if the parameter is absent
- `#[param(default = expr)]` → uses the given expression if absent
- Conversion via `FromStr` for non-String types

### Serde attributes are read, not duplicated

There is no `#[params(...)]` renaming spelling. The derive honours the
`#[serde(...)]` attributes a struct already carries, so a payload shipped as
`Query<T>` migrates untouched:

| Attribute | Effect |
|-----------|--------|
| `#[serde(rename_all = "camelCase")]` (struct) | Renames every field. All serde cases: `lowercase`, `UPPERCASE`, `PascalCase`, `camelCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`, `SCREAMING-KEBAB-CASE` |
| `#[serde(rename = "q")]` (field) | Exact wire name (also `rename(deserialize = "…")`) |
| `#[serde(default)]` / `#[serde(default = "path")]` | Fallback when the parameter is absent |
| `#[serde(skip)]` / `#[serde(skip_deserializing)]` | Never read from the request; built with `Default` |
| `#[serde(flatten)]` (field) | The nested `Params` struct's own keys are read from the same request — identical to a bare `#[params]` |

Precedence: an explicit R2E name (`#[query(name = "q")]`, `#[param(path, name =
"…")]`, `#[header("X-…")]`) > `#[serde(rename)]` > `#[serde(rename_all)]` >
the field identifier. `#[param(default …)]` wins over `#[serde(default …)]`.
`#[serde(skip)]` combined with an R2E param attribute is a compile error, and so is `#[serde(flatten)]` next to `#[query]`/`#[header]`/`#[param]` (a flattened field is a nested group, not a single parameter — use `#[params(prefix = "...")]` to prefix it).

### Migrating `Query<T>` → `Params`

Add `Params` to the derive list and drop the `Query` wrapper in the handler
(keep `Deserialize` if the type is still deserialized elsewhere). The wire
contract is unchanged and every field now appears in the OpenAPI spec:

```rust
// before
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    #[serde(rename = "q")]
    query: String,
    page_size: Option<u32>,
}

#[get("/")]
async fn search(&self, Query(q): Query<SearchQuery>) -> Json<Hits> { /* ... */ }

// after — same `?q=…&pageSize=…`
#[derive(Deserialize, Params)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    #[serde(rename = "q")]
    query: String,
    page_size: Option<u32>,
}

#[get("/")]
async fn search(&self, q: SearchQuery) -> Json<Hits> { /* ... */ }
```

The one visible difference is the 400 body: `Query<T>` answers with serde's
plain text, `Params` with a JSON problem body by default. It is an app-level
setting, resolved once at `build_state()`:

```yaml
server:
  params-rejection-format: json         # default → {"error": "..."}
  # params-rejection-format: plain-text # byte-for-byte `Query<T>` compatibility
```

Handlers that want to inspect a raw rejection themselves can take
`Result<Query<T>, QueryRejection>`: `QueryRejection`, `PathRejection`,
`FormRejection` and `JsonRejection` are re-exported from `r2e::http` (and
`r2e::http::rejection`).

### Usage in a handler

```rust
#[routes]
impl UserController {
    #[get("/{id}")]
    async fn get_user(&self, params: GetUserParams) -> Json<User> {
        // params.id, params.page, params.tenant_id extracted and validated
        let user = self.user_service.find(params.id).await;
        Json(user)
    }
}
```

### OpenAPI integration

`#[derive(Params)]` also generates a `ParamsMetadata` implementation, which feeds parameter metadata (name, location, type, required) into the OpenAPI spec. When a `Params` struct is used as a handler parameter, its fields automatically appear in the generated `/openapi.json` — no manual annotation needed.

## Internal mechanism

The code generated by `#[routes]` uses an autoref specialization mechanism:

1. Deserialization via `Json<T>` (standard Axum)
2. Automatic validation via `__AutoValidator` — if the type derives `Validate`, validation is performed; otherwise, it is a no-op (zero overhead)
3. On failure → 400 response with per-field error details

Types without `#[derive(Validate)]` work normally — no validation is performed.

## Dependencies

```toml
[dependencies]
r2e = "0.3"
garde = { version = "0.23", features = ["derive", "email"] }
```

Validation is always available — no feature flag needed.

## Validation criteria

```bash
# Valid request → 200
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@example.com"}'

# Invalid email → 400
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"not-an-email"}'

# Empty name → 400
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"","email":"alice@example.com"}'
```
