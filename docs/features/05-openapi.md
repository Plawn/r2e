# Feature 5 — OpenAPI

## Objective

Automatically generate an OpenAPI 3.1.0 specification from controller route metadata, and serve a built-in API documentation interface (WTI).

## Key Concepts

### Route metadata

Each controller annotated with `#[derive(Controller)]` + `#[routes]` implements `Controller::route_metadata()`, which returns a list of `RouteInfo` — path, HTTP method, parameters, required roles, request/response schemas.

### OpenApiConfig

Configuration for the specification: title, version, description, documentation UI, extra schemas, and schema overrides.

### OpenApiPlugin

A post-state plugin that registers a meta consumer for `RouteInfo`, builds the spec, and serves `GET /openapi.json` and optionally `GET /docs`.

## Usage

### 1. Add the dependency

```toml
[dependencies]
r2e = { version = "0.1", features = ["openapi"] }
schemars = "1"
```

### 2. Configure and register

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

AppBuilder::new()
    .build_state::<Services, _, _>()
    .await
    .with(OpenApiPlugin::new(
        OpenApiConfig::new("Mon API", "0.1.0")
            .with_description("Description de mon API")
            .with_docs_ui(true),
    ))
    .register_controller::<UserController>()
    .register_controller::<ConfigController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### 3. Generated endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /openapi.json` | OpenAPI 3.1.0 specification in JSON |
| `GET /docs` | API documentation interface (WTI) |
| `GET /docs/wti-element.css` | WTI stylesheet (embedded) |
| `GET /docs/wti-element.js` | WTI script (embedded) |

## Collected metadata

The `#[routes]` macro automatically generates a `RouteInfo` for each route method:

```rust
pub struct RouteInfo {
    pub path: String,           // e.g.: "/users/{id}"
    pub method: String,         // e.g.: "GET"
    pub operation_id: String,   // e.g.: "UserController_get_by_id"
    pub summary: Option<String>,
    pub description: Option<String>,
    pub request_body_type: Option<String>,
    pub request_body_schema: Option<Value>,
    pub response_type: Option<String>,
    pub response_schema: Option<Value>,
    pub response_status: u16,
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    pub tag: Option<String>,
    pub deprecated: bool,
    pub has_auth: bool,
}
```

- Path parameters (e.g.: `Path(id): Path<u64>`) are automatically detected.
- Request body schemas are generated via `schemars::schema_for!(T)` for `Json<T>` parameters.
- Response schemas use autoref specialization — types without `JsonSchema` are silently skipped.
- Doc comments: first `///` line → `summary`, remaining → `description`.
- Roles declared via `#[roles("admin")]` appear in security metadata.

## OpenApiConfig

```rust
let config = OpenApiConfig::new("Titre", "1.0.0")
    .with_description("Description optionnelle")
    .with_docs_ui(true)                     // Enables /docs (default: false)
    .with_schema::<WsMessage>()             // Extra schema from JsonSchema type
    .with_raw_schema("External", json!({    // Manual schema
        "type": "object",
        "properties": { "id": { "type": "string" } }
    }))
    .with_schema_override("ErrorResponse", json!({...}));  // Override auto-generated
```

| Method | Description |
|--------|-------------|
| `new(title, version)` | Create config with title and version |
| `with_description(desc)` | Set API description |
| `with_docs_ui(true)` | Enable interactive docs at `/docs` |
| `with_schema::<T>()` | Register a `JsonSchema` type not in any route |
| `with_raw_schema(name, json)` | Add a manually-crafted JSON schema |
| `with_schema_registry(registry)` | Merge a pre-built `SchemaRegistry` |
| `with_schema_override(name, json)` | Override an auto-generated schema |

### Schema precedence

1. **Overrides** (`with_schema_override`) — highest priority
2. **Route-derived schemas** — from request/response types
3. **Registry schemas** — from `with_schema`, `with_raw_schema`, `with_schema_registry`
4. **Built-in error schemas** — `ErrorResponse`, `ValidationErrorResponse`, `FieldError`

## Documentation interface (WTI)

When `.with_docs_ui(true)` is enabled, the `/docs` endpoint serves an HTML page containing the WTI interface, configured to load `/openapi.json`. The CSS and JS assets are embedded in the binary via `include_str!` and served at `/docs/wti-element.css` and `/docs/wti-element.js`.

The interface allows you to:
- Browse all endpoints
- View parameters and types
- Test endpoints directly from the browser

## Validation criteria

```bash
# OpenAPI spec
curl http://localhost:3000/openapi.json | jq .info.title
# → "Mon API"

# Documentation UI
curl http://localhost:3000/docs | grep "wti-element"
# → HTML containing wti-element
```
