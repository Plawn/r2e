# Feature 5 — OpenAPI

## TL;DR

Generate an OpenAPI 3.1.0 spec from controller route metadata and serve a built-in docs UI. Enable the `openapi` feature (plus `schemars`), then add `.plugin(OpenApiPlugin::new(OpenApiConfig::new("My API", "0.1.0").with_docs_ui(true)))` before `build_state()`. The `#[routes]` macro emits a `RouteInfo` per route (path, method, params, roles, request/response schemas). Serves `GET /openapi.json` and `GET /docs`.


## Objective

Automatically generate an OpenAPI 3.1.0 specification from controller route metadata, and serve a built-in API documentation interface (WTI).

## Key Concepts

### Route metadata

Each controller annotated with `#[controller]` + `#[routes]` implements `Controller::register_meta(&mut MetaRegistry)`, which pushes one `RouteInfo` per route — path, HTTP method, parameters, required roles, request/response schemas. The plugin reads them from `RoutesContext::routes()` in a Routes-stage effect, which runs after every controller (and every plugin-shipped controller) has registered.

### OpenApiConfig

Configuration for the specification: title, version, description, documentation UI, extra schemas, and schema overrides.

### OpenApiPlugin

A plugin that reads the collected `RouteInfo` set in a **Routes**-stage effect (so it sees every controller regardless of install order), builds the spec, and serves `GET /openapi.json` and optionally `GET /docs`.

## Usage

### 1. Add the dependency

```toml
[dependencies]
r2e = { version = "0.3", features = ["openapi"] }
schemars = "1"
```

### 2. Configure and register

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

AppBuilder::new()
    .register::<UserService>()      // register the beans your controllers inject
    .plugin(OpenApiPlugin::new(
        OpenApiConfig::new("Mon API", "0.1.0")
            .with_description("Description de mon API")
            .with_docs_ui(true),
    ))
    .build_state()                  // no type args — the state is the inferred HList
    .await
    .register_controllers::<(UserController, ConfigController)>()
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
    pub request_body_content_type: Option<String>,  // None ⇒ application/json
    pub request_body_required: bool,
    pub response_type: Option<String>,
    pub response_schema: Option<Value>,
    pub response_status: u16,
    pub response_unmapped: Option<String>,  // successful body type that could not be auto-mapped
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    pub tag: Option<String>,
    pub deprecated: bool,
    pub rejection_kinds: Vec<RejectionKind>,     // what the route can fail with (inferred)
    pub error_schema: Option<ErrorSchemaInfo>,   // envelope of a `Result<T, E>` return, else app-level
}
```

- Path parameters (e.g.: `Path(id): Path<u64>`) are automatically detected.
- Request body schemas are generated via `schemars::schema_for!(T)` for `Json<T>` parameters.
- Response schemas use autoref specialization — types without `JsonSchema` are silently skipped.
- Doc comments: first `///` line → `summary`, remaining → `description`.
- Roles declared via `#[roles("admin")]` appear in security metadata.
- Error responses are derived from `rejection_kinds` (see below), never hardcoded.

## Error responses

The macro infers the `RejectionKind`s a route can fail with — the body
extractor's failures (`Json<T>`: `MissingContentType` 415, `PayloadTooLarge`
413, `BodyRead`/`MalformedBody` 400, `InvalidBody` 422), `Path<T>` →
`InvalidPath`, `Query<T>` / `#[derive(Params)]` fields → `InvalidQuery` /
`InvalidHeader`, a `Form<T>` body → `UnsupportedMediaType` / `PayloadTooLarge`
/ `BodyRead` / `InvalidBody` (422), a GET `Form<T>` (query string) →
`InvalidForm`, a garde `Validate` parameter → `Validation`, an identity
(struct-level or parameter, required or `Option<..>` — a present but invalid
token still fails) → `Unauthenticated`, `#[roles]` / guards → `Forbidden`, a
rate-limit guard → `RateLimited`, and always `Internal`. Controller-level
guards fold in for non-`#[anonymous]` routes, pre-auth guards for every route.
SSE and WS routes are inferred from their parameters by the same rules.

The builder documents one response per distinct status the route's **error
envelope** maps those kinds to, through `ErrorSchema::status_of(kind)`, with
`body_schema_for(kind)` (else `body_schema()`) as the component, plus the
envelope's `extra_statuses()`. The envelope is, in order: the handler's
`Result<T, E>` error type when `E: ErrorSchema` (`RouteInfo::error_schema`),
the application's `AppBuilder::error_projection::<E>()` (the plugin reads the
`ErrorProjector` bean; `OpenApiConfig::with_error_schema::<E>()` does the same
for direct `build_spec` callers), else `HttpError` — `ErrorResponse`
everywhere, `ValidationErrorResponse` (inline `details` items) for
`Validation`. Bodies are deduplicated by schema — the same body under two
names is documented once, under the first-recorded (route envelope's) name —
and distinct bodies on one status render as an `anyOf` (a validation body is
also a valid plain error body, which a `oneOf` would reject). Runtime and
spec share `status_of`, so a remap (422 → 400) is documented as 400 only.

Every route also documents the **panic 500** with the application envelope's
body: the catch-panic layer renders through the `ErrorProjector`, never the
route's envelope. When the route's own envelope puts a different body on 500
the two render as an `anyOf`; when it remaps `Internal` elsewhere (say 503) the
spec lists both 500 (application body) and 503 (route body). Nested types in
an envelope body schema (`$defs`) are promoted to `components/schemas`.

Route, registry and nested `$defs` schemas own their component names. An
error body named like a different schema already there — say a success DTO
nesting its own `ErrorResponse`, or a route envelope reusing `HttpError`'s
`ErrorResponse` with another shape — keeps the existing component and is
documented **inline** in its responses; spec generation warns once at boot
(`SchemaGap::ErrorBodyInlined`, naming the route and the component).
`build_spec_with_warnings(&config, &routes)` returns the spec with every
warning instead of logging them.

A custom body extractor — the handler's last parameter, read with
`FromRequest` — is documented when it implements
`r2e_core::di::meta::RequestBodySchema` (`content_type()`, `body_schema()`,
`rejection_kinds()`). Without it the route has no request body in the spec.

A custom **response** type — any concrete return type other than `Json<T>`,
the `T` of a `Result<T, E>` included — is documented when it implements
`r2e_core::di::meta::ResponseBodySchema`: `response_contents()` lists every media type it
can be served as (`ResponseContent::json(..)`, `::event_stream(..)`,
`::text()`, `::new(ct, ..)`), each with an optional `(component name, schema)`
— `r2e_openapi::schema_of::<T>()` builds one from a `JsonSchema` type. A handler
answering JSON or an SSE stream depending on the request returns an enum and
documents both under its success status:

```rust,ignore
enum ChatReply {
    Json(Json<Completion>),
    Stream(Sse<BoxStream<'static, Result<SseEvent, Infallible>>>),
}

impl ResponseBodySchema for ChatReply {
    fn response_contents() -> Vec<ResponseContent> {
        vec![
            ResponseContent::json(Some(schema_of::<Completion>())),
            ResponseContent::event_stream(Some(schema_of::<Chunk>())),
        ]
    }
}
```

`text/*` media types without a schema render as `{"type": "string"}`. Return
types containing `impl Trait` are never probed.

## Tags

`tag` defaults to the controller's struct name. `#[controller(path = "…", tag =
"…")]` sets it explicitly, which is how several controllers publish under a
single tag — the grouping in the spec (and in Swagger UI) stops being welded to
the Rust type name:

```rust
#[controller(path = "/catalog/items", tag = "Catalog")]
pub struct CatalogItemsController;

#[controller(path = "/catalog/categories", tag = "Catalog")]
pub struct CatalogCategoriesController;   // same "Catalog" group
```

The tag is a `const OPENAPI_TAG` in the controller's generated meta module, so
every `RouteInfo` the controller pushes carries it — no per-route annotation.

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
| `with_error_schema::<E>()` | Document errors with envelope `E` (the plugin sets it from `ErrorProjector`) |

### Schema precedence

1. **Overrides** (`with_schema_override`) — highest priority
2. **Route-derived schemas** — from request/response types
3. **Registry schemas** — from `with_schema`, `with_raw_schema`, `with_schema_registry`
4. **Error envelope schemas** — whatever the routes' envelopes declare
   (`ErrorResponse` / `ValidationErrorResponse` for `HttpError`)

## Documentation interface (WTI)

When `.with_docs_ui(true)` is enabled, the `/docs` endpoint serves an HTML page containing the WTI (`<wti-element>`) web component, configured to load `/openapi.json`. The CSS and JS assets are embedded in the binary via `include_str!` and served at `/docs/wti-element.css` and `/docs/wti-element.js` — no external CDN, no network access required at runtime.

The interface allows you to:
- Browse all endpoints
- View parameters and types
- Test endpoints directly from the browser

### `<wti-element>` attributes

The served HTML wires the component with a fixed configuration. The component itself supports:

| Attribute | Values | Default | Notes |
|-----------|--------|---------|-------|
| `spec-url` | URL | — | Points at `/openapi.json` (set by R2E). |
| `spec-type` | `openapi`, `grpc` | `openapi` | R2E serves an OpenAPI spec. |
| `theme` | `light`, `dark` | `light` | R2E's page pins `dark`. |
| `locale` | `en`, `fr` | `en` | R2E's page pins `en`. |

The bundled assets are vendored from the [WTI](https://github.com/plawn/wti) monorepo (`packages/web-component`, currently **v0.3.1**). To refresh them, rebuild `wti-element` there (`bun run build` in `packages/web-component`) and copy `dist/wti-element.iife.js` + `dist/wti-element.css` into `r2e-openapi/assets/`.

## Validation criteria

```bash
# OpenAPI spec
curl http://localhost:3000/openapi.json | jq .info.title
# → "Mon API"

# Documentation UI
curl http://localhost:3000/docs | grep "wti-element"
# → HTML containing wti-element
```
