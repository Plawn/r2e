# r2e-openapi

OpenAPI 3.1.0 spec generation for R2E — automatic API documentation with interactive UI.

## Overview

Generates an OpenAPI 3.1.0 specification from route metadata collected during controller registration. Optionally serves an interactive API documentation UI.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["openapi"] }
schemars = "1"
```

## Setup

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

let openapi = OpenApiConfig::new("My API", "1.0.0")
    .with_description("REST API documentation")
    .with_docs_ui(true);

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(OpenApiPlugin::new(openapi))
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Endpoints

| Path | Description |
|------|-------------|
| `/openapi.json` | OpenAPI 3.1.0 JSON specification |
| `/docs` | Interactive API documentation UI (when `docs_ui` is enabled) |

## Extra schemas

Route request/response types are included automatically. Use the schema methods for types not referenced by any route:

```rust
// Register a JsonSchema type
OpenApiConfig::new("My API", "1.0.0")
    .with_schema::<WsMessage>()
    .with_schema::<DomainEvent>()

// Manual schema (no JsonSchema derive needed)
    .with_raw_schema("Legacy", json!({"type": "object", "properties": {"id": {"type": "string"}}}))

// Override an auto-generated schema
    .with_schema_override("ErrorResponse", json!({...}))

// Bulk registration
    .with_schema_registry(registry)
```

Precedence: overrides > route schemas > registry > built-in error schemas.

## How it works

1. Route metadata is collected from `Controller::route_metadata()` during `register_controller()`
2. Request/response schemas are auto-generated via `schemars` at compile time
3. Extra schemas from `SchemaRegistry` are merged (with `$defs` promotion and `$ref` rewriting)
4. The spec is assembled and served as a JSON endpoint
5. When `docs_ui` is enabled, an interactive UI is served at `/docs`

## License

Apache-2.0
