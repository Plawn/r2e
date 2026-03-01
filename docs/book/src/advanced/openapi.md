# OpenAPI

R2E auto-generates an OpenAPI 3.1.0 specification from your controller route metadata, with an optional interactive documentation UI.

## Setup

**1. Enable the openapi feature:**

```toml
[dependencies]
r2e = { version = "0.1", features = ["openapi"] }
```

**2. Add schemars for request/response schemas:**

```toml
[dependencies]
schemars = "1"
```

> `schemars` must be a **direct dependency** in your `Cargo.toml`. This is a Rust
> limitation shared by all derive-macro crates (same pattern as `serde`, `garde`,
> etc.) — the `#[derive(JsonSchema)]` proc macro generates code that references
> the `schemars` crate by name, so the compiler must be able to resolve it from
> your crate root.

**3. Derive `JsonSchema` on request/response types:**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, JsonSchema)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}
```

**4. Register the OpenAPI plugin:**

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(OpenApiPlugin::new(
        OpenApiConfig::new("My API", "1.0.0")
            .with_description("API description")
            .with_docs_ui(true),
    ))
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /openapi.json` | OpenAPI 3.1.0 specification (always served) |
| `GET /docs` | Interactive API documentation (if `with_docs_ui(true)`) |

## What gets documented

Route metadata is automatically collected from `Controller::route_metadata()` during `register_controller()`:

| Feature | Source |
|---------|--------|
| Paths | `#[controller(path = "...")]` + `#[get("/...")]` |
| HTTP methods | `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` |
| Operation IDs | Handler method names |
| Request body schemas | `Json<T>` parameters where `T: JsonSchema` |
| Response schemas | Return type analysis (`Json<T>`, `JsonResult<T>`, `Result<Json<T>, _>`) |
| Path/query/header params | `Path`, `Query`, `#[derive(Params)]` |
| Required roles | `#[roles("admin", "editor")]` |
| Summary | First line of `///` doc comment |
| Description | Remaining lines of `///` doc comment |
| Deprecated | `#[deprecated]` (standard Rust attribute) |
| Status codes | Smart defaults (GET→200, POST→201, DELETE→204) or `#[status(N)]` |
| Auth responses | 401/403 auto-added only for authenticated routes |

## Route attributes for OpenAPI

### `#[status(N)]` — Override default status code

```rust
#[post("/users")]
#[status(201)]
async fn create(&self, body: Json<CreateUser>) -> JsonResult<User> { ... }
```

Default status codes: GET/PUT/PATCH → 200, POST → 201, DELETE → 204.

### `#[returns(T)]` — Explicit response type

Use when the return type is opaque (e.g., `impl IntoResponse`):

```rust
#[get("/widgets/{id}")]
#[returns(Widget)]
async fn get_widget(&self, Path(id): Path<u64>) -> impl IntoResponse { ... }
```

### `#[deprecated]` — Mark as deprecated in the spec

```rust
/// Old endpoint
#[get("/v1/users")]
#[deprecated]
async fn list_v1(&self) -> JsonResult<Vec<User>> { ... }
```

### Doc comments — Summary and description

```rust
/// List all users                              ← summary (first line)
///
/// Returns a paginated list of active users.   ← description (rest)
#[get("/users")]
async fn list(&self) -> JsonResult<Vec<User>> { ... }
```

### Optional request body

`Option<Json<T>>` is detected as `required: false` in the spec:

```rust
#[put("/users/{id}")]
async fn update(&self, Path(id): Path<u64>, body: Option<Json<PatchUser>>) -> JsonResult<User> {
    ...
}
```

## Return type detection

The macro automatically detects the response type from common patterns:

| Return type | Detected response |
|-------------|-------------------|
| `Json<T>` | Schema for `T` |
| `JsonResult<T>` | Schema for `T` |
| `Result<Json<T>, HttpError>` | Schema for `T` |
| `ApiResult<Json<T>>` | Schema for `T` |
| `StatusCode` / `StatusResult` | No body |
| `String` | No schema |
| `impl IntoResponse` | Use `#[returns(T)]` |

> **Note:** Response schemas are generated via autoref specialization — if `T`
> does not implement `JsonSchema`, the schema is silently omitted (no compile
> error). Add `#[derive(JsonSchema)]` to your response types to see them in
> the spec.

## Full example

```rust
use r2e::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
pub struct CreateUser {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, JsonSchema)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject] user_service: UserService,
}

#[routes]
impl UserController {
    /// List all users
    ///
    /// Returns all users in the system.
    #[get("/")]
    async fn list(&self) -> JsonResult<Vec<User>> {
        Ok(Json(self.user_service.list().await))
    }

    /// Create a new user
    #[post("/")]
    #[roles("admin")]
    async fn create(&self, body: Json<CreateUser>) -> JsonResult<User> {
        Ok(Json(self.user_service.create(body.0).await?))
    }

    /// Delete a user
    #[delete("/{id}")]
    #[roles("admin")]
    async fn delete(&self, Path(id): Path<u64>) -> StatusResult {
        self.user_service.delete(id).await?;
        Ok(StatusCode::NO_CONTENT)
    }
}
```

This produces a spec with:
- `POST /users` → 201 with `User` schema, `CreateUser` request body, 401/403 responses
- `GET /users` → 200 with `Vec<User>` schema
- `DELETE /users/{id}` → 204 no body, 401/403 responses
- Summaries and descriptions from doc comments
- All schemas under `components/schemas`

## OpenApiConfig options

| Method | Description |
|--------|-------------|
| `new(title, version)` | Create config with title and version |
| `with_description(desc)` | Set API description |
| `with_docs_ui(true)` | Enable interactive docs at `/docs` |
