# OpenAPI

R2E auto-generates an OpenAPI 3.0.3 specification from your controller route metadata, with an optional interactive documentation UI.

## Setup

Enable the openapi feature:

```toml
r2e = { version = "0.1", features = ["openapi"] }
```

## Configuration

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

AppBuilder::new()
    .build_state::<AppState, _>()
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
| `GET /openapi.json` | OpenAPI 3.0.3 specification |
| `GET /docs` | Interactive API documentation (if `with_docs_ui(true)`) |

## What gets documented

Route metadata is automatically collected from `Controller::route_metadata()` during `register_controller()`:

- **Paths** — from `#[controller(path = "...")]` + `#[get("/...")]`
- **HTTP methods** — GET, POST, PUT, DELETE, PATCH
- **Path parameters** — from `{param}` in paths
- **Required roles** — from `#[roles("...")]`
- **Operation IDs** — from handler method names

## OpenApiConfig options

| Method | Description |
|--------|-------------|
| `new(title, version)` | Create config with title and version |
| `with_description(desc)` | Set API description |
| `with_docs_ui(true)` | Enable interactive docs at `/docs` |

## Example spec output

```json
{
  "openapi": "3.0.3",
  "info": {
    "title": "My API",
    "version": "1.0.0",
    "description": "API description"
  },
  "paths": {
    "/users": {
      "get": {
        "operationId": "list",
        "parameters": [],
        "responses": {
          "200": {
            "description": "Successful response"
          }
        }
      },
      "post": {
        "operationId": "create",
        "responses": {
          "200": {
            "description": "Successful response"
          }
        },
        "security": [{"roles": ["admin"]}]
      }
    },
    "/users/{id}": {
      "get": {
        "operationId": "get_by_id",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "schema": {"type": "string"}
          }
        ]
      }
    }
  }
}
```
