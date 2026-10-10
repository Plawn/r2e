# OpenAPI

R2E auto-generates an OpenAPI 3.1.0 specification from your controller route metadata, with an optional interactive documentation UI.

## Setup

**1. Enable the openapi feature:**

```toml
[dependencies]
r2e = { version = "0.3", features = ["openapi"] }
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
    .plugin(OpenApiPlugin::new(
        OpenApiConfig::new("My API", "1.0.0")
            .with_description("API description")
            .with_docs_ui(true),
    ))
    .build_state()
    .await
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

Route metadata is automatically collected via `Controller::register_meta()` during `register_controller()`:

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
| Error responses | Inferred from the route's `RejectionKind`s × its error envelope's `status_of` — see [Error responses](#error-responses) |

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

## Error responses

Error responses come from what the route can actually fail with, not from a
fixed list. The `#[routes]` macro records the route's `RejectionKind`s in
`RouteInfo::rejection_kinds`:

| Route shape | Inferred kinds (default status) |
|-------------|--------------------------------|
| JSON or `Form<T>` body | `MissingContentType`/`UnsupportedMediaType` (415), `PayloadTooLarge` (413), `MalformedBody` (400), `InvalidBody` (422) |
| GET `Form<T>` | `InvalidForm` (400) |
| `Path<T>` | `InvalidPath` (400) |
| `Query<T>` / `#[derive(Params)]` | `InvalidQuery` (400) |
| garde `Validate` parameter | `Validation` (400) |
| identity — required or `Option<..>` | `Unauthenticated` (401) |
| `#[roles]` / guards | `Forbidden` (403) |
| rate-limit guard | `RateLimited` (429) |
| always | `Internal` (500) |

SSE and WebSocket routes are inferred from their parameters the same way.

The builder then emits **one response per distinct status** the route's error
envelope maps those kinds to (`ErrorSchema::status_of`), with the envelope's
`body_schema` / `body_schema_for(kind)` as the component, plus its
`extra_statuses`. The envelope is the handler's `Result<T, E>` error type when
it is one, else the application's (`AppBuilder::error_projection::<E>()`, read
from the `ErrorProjector` bean by the plugin; `OpenApiConfig::with_error_schema::<E>()`
for direct `build_spec` callers), else `HttpError` (`ErrorResponse` /
`ValidationErrorResponse`). Runtime and spec call the same `status_of`, so an
envelope remapping `Validation` to 422 documents 422 — never both. See
[Rejection, envelopes and projection](../core-concepts/error-handling.md#rejection-envelopes-and-projection).

Rules the builder applies:

- **Dedup by schema.** The same body under two names is documented once, under
  the first-recorded name (the route envelope's).
- **`anyOf`, never `oneOf`.** Distinct bodies on one status render as `anyOf`
  (a validation body is also a valid plain error body, so `oneOf` would reject
  it).
- **Panic 500 uses the application envelope.** The catch-panic layer renders
  through the `ErrorProjector`, never the route's envelope, so every route
  documents the 500 with the app envelope's body — an `anyOf` beside the route's
  own 500 body when they differ.
- **Name collisions are inlined.** An error body whose component name is already
  taken by a different schema (a DTO, a registry entry, a nested `$defs` type,
  another envelope's body) leaves that component alone and is documented inline,
  with a boot warning (`SchemaGap::ErrorBodyInlined`).
- Nested types in an envelope body schema are promoted to `components/schemas`
  like any other schema.

A custom body extractor (the handler's last parameter, read with `FromRequest`)
is documented when it implements `r2e::di::meta::RequestBodySchema`
(`content_type()`, `body_schema()`, `rejection_kinds()`); otherwise the route
has no request body in the spec.

The demo app's `POST /problems/` (`Result<Json<Ticket>, Problem>`) documents
400/401/413/415/422/500 with the `Problem` component, the 500 as an `anyOf` of
`Problem` and the app-level `ErrorResponse`; its infallible neighbour
`GET /problems/legacy/{id}` documents a 400 `ErrorResponse` only. The test
`openapi_documents_the_problem_envelope_per_route` in
`examples/example-app/tests/http/error_envelope.rs` pins this.

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

#[controller(path = "/users")]
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
- `POST /users` → 201 with `User` schema, `CreateUser` request body; error responses inferred from the route — 403 (`#[roles]`), 400/413/415/422 (JSON body), 500 — all with the default `ErrorResponse` body
- `GET /users` → 200 with `Vec<User>` schema
- `DELETE /users/{id}` → 204 no body; 400 (`Path`), 403 (`#[roles]`) and 500 error responses
- Summaries and descriptions from doc comments
- All schemas under `components/schemas`

## OpenApiConfig options

| Method | Description |
|--------|-------------|
| `new(title, version)` | Create config with title and version |
| `with_description(desc)` | Set API description |
| `with_docs_ui(true)` | Enable interactive docs at `/docs` |
| `with_schema::<T>()` | Register an extra schema for `T: JsonSchema` |
| `with_raw_schema(name, json)` | Add a manually-crafted JSON schema |
| `with_schema_registry(registry)` | Merge a pre-built `SchemaRegistry` |
| `with_schema_override(name, json)` | Override an auto-generated schema |

## Extra schemas

Route request/response types are included automatically. Use the schema methods for types that don't appear in any route but should still be in the spec (WebSocket messages, domain events, shared DTOs, etc.).

### Register a `JsonSchema` type

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

OpenApiConfig::new("My API", "1.0.0")
    .with_schema::<WsMessage>()
    .with_schema::<DomainEvent>()
    .with_docs_ui(true)
```

### Manual schema (no `JsonSchema` derive)

```rust
use serde_json::json;

OpenApiConfig::new("My API", "1.0.0")
    .with_raw_schema("ExternalThing", json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "status": { "type": "string", "enum": ["active", "inactive"] }
        }
    }))
```

### Override an auto-generated schema

Override takes precedence over both route-derived and registry schemas:

```rust
OpenApiConfig::new("My API", "1.0.0")
    .with_schema_override("ErrorResponse", json!({
        "type": "object",
        "properties": {
            "code": { "type": "integer" },
            "message": { "type": "string" },
            "details": { "type": "array", "items": { "type": "string" } }
        }
    }))
```

### Bulk registration with `SchemaRegistry`

```rust
use r2e::r2e_openapi::{SchemaRegistry, OpenApiConfig};

let mut registry = SchemaRegistry::new();
registry.register_for::<WsMessage>();
registry.register_for::<DomainEvent>();
registry.register("Legacy", json!({"type": "object"}));

OpenApiConfig::new("My API", "1.0.0")
    .with_schema_registry(registry)
```

### Schema precedence

When the same name appears from multiple sources:

1. **Overrides** (`with_schema_override`) — highest priority
2. **Route-derived schemas** — from request/response types
3. **Registry schemas** — from `with_schema`, `with_raw_schema`, `with_schema_registry`
4. **Built-in error schemas** — `ErrorResponse`, `ValidationErrorResponse`, `FieldError`
