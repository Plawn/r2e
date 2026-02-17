# r2e-openapi

OpenAPI 3.0 spec generation for R2E — automatic API documentation with Swagger UI.

## Overview

Generates an OpenAPI 3.0.3 specification from route metadata collected during controller registration. Optionally serves an interactive API documentation UI.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["openapi"] }
```

## Setup

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

let openapi = OpenApiConfig::new("My API", "1.0.0")
    .with_description("REST API documentation")
    .with_docs_ui(true);

AppBuilder::new()
    .build_state::<AppState, _>()
    .await
    .with(OpenApiPlugin::new(openapi))
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Endpoints

| Path | Description |
|------|-------------|
| `/openapi.json` | OpenAPI 3.0.3 JSON specification |
| `/docs` | Interactive API documentation UI (when `docs_ui` is enabled) |

## How it works

1. Route metadata is collected from `Controller::route_metadata()` during `register_controller()`
2. `SchemaRegistry` collects JSON Schemas for request/response types via `SchemaProvider`
3. The spec is assembled and served as a JSON endpoint
4. When `docs_ui` is enabled, an interactive Swagger-like UI is served at `/docs`

## License

Apache-2.0
