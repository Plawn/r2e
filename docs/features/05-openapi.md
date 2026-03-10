# Feature 5 — OpenAPI

## Objective

Automatically generate an OpenAPI 3.0.3 specification from controller route metadata, and serve a built-in API documentation interface (WTI).

## Key Concepts

### Route metadata

Each controller annotated with `#[derive(Controller)]` + `#[routes]` implements `Controller::route_metadata()`, which returns a list of `RouteInfo` — path, HTTP method, parameters, required roles.

### OpenApiConfig

Configuration for the specification: title, version, description, and whether to enable the documentation interface.

### openapi_routes()

A function that takes an `OpenApiConfig` and the metadata from all controllers, and returns a `Router` with the `/openapi.json` and `/docs` endpoints.

## Usage

### 1. Add the dependency

```toml
[dependencies]
r2e-openapi = { path = "../r2e-openapi" }
```

### 2. Configure and register

```rust
use r2e_core::Controller;
use r2e_openapi::{openapi_routes, OpenApiConfig};

let openapi_config = OpenApiConfig::new("Mon API", "0.1.0")
    .with_description("Description de mon API")
    .with_docs_ui(true);

let openapi = openapi_routes::<Services>(
    openapi_config,
    vec![
        UserController::route_metadata(),
        ConfigController::route_metadata(),
        DataController::route_metadata(),
    ],
);

AppBuilder::new()
    .with_state(services)
    .register_controller::<UserController>()
    .register_controller::<ConfigController>()
    .register_controller::<DataController>()
    .register_routes(openapi)  // Adds /openapi.json and /docs
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### 3. Generated endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /openapi.json` | OpenAPI 3.0.3 specification in JSON |
| `GET /docs` | API documentation interface (WTI) |
| `GET /docs/wti-element.css` | WTI stylesheet (embedded) |
| `GET /docs/wti-element.js` | WTI script (embedded) |

### 4. Example of generated spec

```json
{
    "openapi": "3.0.3",
    "info": {
        "title": "Mon API",
        "version": "0.1.0",
        "description": "Description de mon API"
    },
    "paths": {
        "/users": {
            "get": {
                "operationId": "UserController_list",
                "responses": {
                    "200": { "description": "Success" }
                }
            },
            "post": {
                "operationId": "UserController_create",
                "responses": {
                    "200": { "description": "Success" }
                }
            }
        },
        "/users/{id}": {
            "get": {
                "operationId": "UserController_get_by_id",
                "parameters": [
                    {
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "schema": { "type": "string" }
                    }
                ],
                "responses": {
                    "200": { "description": "Success" }
                }
            }
        }
    }
}
```

## Collected metadata

The `#[routes]` macro automatically generates a `RouteInfo` for each route method:

```rust
pub struct RouteInfo {
    pub path: String,           // e.g.: "/users/{id}"
    pub method: String,         // e.g.: "GET"
    pub operation_id: String,   // e.g.: "UserController_get_by_id"
    pub summary: Option<String>,
    pub request_body_type: Option<String>,
    pub response_type: Option<String>,
    pub params: Vec<ParamInfo>, // Detected path parameters
    pub roles: Vec<String>,     // Required roles (#[roles("admin")])
}
```

Path parameters (e.g.: `Path(id): Path<u64>`) are automatically detected and included in the spec.

Roles declared via `#[roles("admin")]` appear in the metadata, allowing the documentation interface to display them.

## OpenApiConfig

```rust
let config = OpenApiConfig::new("Titre", "1.0.0")
    .with_description("Description optionnelle")
    .with_docs_ui(true);   // Enables /docs (default: false)
```

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
