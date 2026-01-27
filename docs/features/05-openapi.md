# Feature 5 — OpenAPI

## Objectif

Generer automatiquement une specification OpenAPI 3.0.3 a partir des metadonnees de route des controllers, et servir une interface de documentation API integree (WTI).

## Concepts cles

### Route metadata

Chaque controller genere par la macro `controller!` implemente `Controller::route_metadata()` qui retourne la liste des `RouteInfo` — chemin, methode HTTP, parametres, roles requis.

### OpenApiConfig

Configuration de la specification : titre, version, description, activation de l'interface de documentation.

### openapi_routes()

Fonction qui prend un `OpenApiConfig` et les metadonnees de tous les controllers, et retourne un `Router` avec les endpoints `/openapi.json` et `/docs`.

## Utilisation

### 1. Ajouter la dependance

```toml
[dependencies]
quarlus-openapi = { path = "../quarlus-openapi" }
```

### 2. Configurer et enregistrer

```rust
use quarlus_core::Controller;
use quarlus_openapi::{openapi_routes, OpenApiConfig};

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
    .register_routes(openapi)  // Ajoute /openapi.json et /docs
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### 3. Endpoints generes

| Endpoint | Description |
|----------|-------------|
| `GET /openapi.json` | Specification OpenAPI 3.0.3 en JSON |
| `GET /docs` | Interface de documentation API (WTI) |
| `GET /docs/wti-element.css` | Stylesheet WTI (embarque) |
| `GET /docs/wti-element.js` | Script WTI (embarque) |

### 4. Exemple de spec generee

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

## Metadonnees collectees

La macro `controller!` genere automatiquement un `RouteInfo` pour chaque methode de route :

```rust
pub struct RouteInfo {
    pub path: String,           // ex: "/users/{id}"
    pub method: String,         // ex: "GET"
    pub operation_id: String,   // ex: "UserController_get_by_id"
    pub summary: Option<String>,
    pub request_body_type: Option<String>,
    pub response_type: Option<String>,
    pub params: Vec<ParamInfo>, // Parametres de chemin detectes
    pub roles: Vec<String>,     // Roles requis (#[roles("admin")])
}
```

Les parametres de chemin (ex: `Path(id): Path<u64>`) sont automatiquement detectes et inclus dans la spec.

Les roles declares via `#[roles("admin")]` apparaissent dans les metadonnees, permettant a l'interface de documentation de les afficher.

## OpenApiConfig

```rust
let config = OpenApiConfig::new("Titre", "1.0.0")
    .with_description("Description optionnelle")
    .with_docs_ui(true);   // Active /docs (defaut: false)
```

## Interface de documentation (WTI)

Quand `.with_docs_ui(true)` est active, l'endpoint `/docs` sert une page HTML contenant l'interface WTI, configuree pour charger `/openapi.json`. Les assets CSS et JS sont embarques dans le binaire via `include_str!` et servis sur `/docs/wti-element.css` et `/docs/wti-element.js`.

L'interface permet de :
- Parcourir tous les endpoints
- Voir les parametres et types
- Tester les endpoints directement depuis le navigateur

## Critere de validation

```bash
# Spec OpenAPI
curl http://localhost:3000/openapi.json | jq .info.title
# → "Mon API"

# Documentation UI
curl http://localhost:3000/docs | grep "wti-element"
# → HTML contenant wti-element
```
