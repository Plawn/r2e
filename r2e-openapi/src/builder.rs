use r2e_core::di::meta::{ParamLocation, RouteInfo};
use r2e_core::{ErrorSchema, ErrorSchemaInfo, HttpError, RejectionKind};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

use crate::schema::SchemaRegistry;

/// Recursively rewrite `$ref` paths from schemars format to OpenAPI components format.
///
/// schemars 1.x generates JSON Schema Draft 2020-12 using `$defs` and
/// `$ref: "#/$defs/X"`. OpenAPI 3.1.0 expects schemas under `#/components/schemas/X`.
fn sanitize_schema(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            if let Some(Value::String(ref_str)) = obj.get_mut("$ref") {
                if ref_str.starts_with("#/$defs/") {
                    *ref_str = ref_str.replace("#/$defs/", "#/components/schemas/");
                }
            }

            for (_, v) in obj.iter_mut() {
                sanitize_schema(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                sanitize_schema(v);
            }
        }
        _ => {}
    }
}

/// Strip `$schema`, move the schema's `$defs` into `extra_definitions`
/// (promoted to `components/schemas` by `build_spec`) and rewrite its
/// `#/$defs/X` references to `#/components/schemas/X`.
///
/// Every schema that lands in `components/schemas` goes through this — route
/// bodies, registry entries and error-envelope bodies alike — so a rewritten
/// reference always has a component to resolve to.
fn promote_defs(mut schema: Value, extra_definitions: &mut Vec<(String, Value)>) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
        // schemars 1.x uses "$defs" (Draft 2020-12)
        if let Some(Value::Object(defs)) = obj.remove("$defs") {
            for (def_name, def_schema) in defs {
                extra_definitions.push((def_name, def_schema));
            }
        }
    }
    sanitize_schema(&mut schema);
    schema
}

/// Insert a schema into the schemas map, promoting `$defs` to top-level components.
fn insert_schema(
    schemas: &mut Map<String, Value>,
    extra_definitions: &mut Vec<(String, Value)>,
    type_name: &str,
    root_schema: &Option<Value>,
) {
    if let Some(root) = root_schema {
        let schema = promote_defs(root.clone(), extra_definitions);
        schemas.insert(type_name.to_string(), schema);
    } else {
        schemas.insert(type_name.to_string(), json!({ "type": "object" }));
    }
}

/// A gap between a route and its generated OpenAPI schema, surfaced as a
/// once-at-boot warning so silently-undocumented bodies become visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaGap {
    /// A successful response carries a body whose Rust return type could not be
    /// mapped to a schema (an `impl Trait` return, or a concrete non-`Json`
    /// type). The response is documented **without a body**.
    MissingResponseBody { type_name: String },
    /// The named response type is documented, but it has no
    /// `schemars::JsonSchema`, so its body renders as a generic `object`.
    SchemalessResponseBody { type_name: String },
    /// The named request type is documented, but it has no
    /// `schemars::JsonSchema`, so its body renders as a generic `object`.
    SchemalessRequestBody { type_name: String },
    /// An error body of the route's envelope is named like a different
    /// schema already in `components/schemas` (a DTO, a registry entry, a
    /// promoted `$defs` type or another envelope's body). That schema keeps
    /// the component; the error body is documented **inline** in the
    /// route's responses.
    ErrorBodyInlined { component: String },
}

/// A single OpenAPI spec-generation warning: a [`SchemaGap`] tied to the route
/// that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecWarning {
    pub method: String,
    pub path: String,
    pub gap: SchemaGap,
}

impl SpecWarning {
    /// A human-readable, actionable warning message naming the route, the
    /// offending type, and how to fix it.
    pub fn message(&self) -> String {
        match &self.gap {
            SchemaGap::MissingResponseBody { type_name } => format!(
                "OpenAPI: {} {} — response body (return type `{}`) could not be mapped to a schema; \
                 the response is documented without a body. Return `Json<T>` (with \
                 `T: schemars::JsonSchema`) or annotate the handler with `#[returns(T)]`.",
                self.method, self.path, type_name
            ),
            SchemaGap::SchemalessResponseBody { type_name } => format!(
                "OpenAPI: {} {} — response type `{}` does not implement `schemars::JsonSchema`; \
                 it is documented as a generic `object`. Derive `schemars::JsonSchema` on `{}`.",
                self.method, self.path, type_name, type_name
            ),
            SchemaGap::SchemalessRequestBody { type_name } => format!(
                "OpenAPI: {} {} — request type `{}` does not implement `schemars::JsonSchema`; \
                 it is documented as a generic `object`. Derive `schemars::JsonSchema` on `{}`.",
                self.method, self.path, type_name, type_name
            ),
            SchemaGap::ErrorBodyInlined { component } => format!(
                "OpenAPI: {} {} — error body `{}` collides with a different schema of the same \
                 name in components/schemas; the error body is documented inline. Rename the \
                 envelope's body (`ErrorSchema::body_schema`) or the colliding type.",
                self.method, self.path, component
            ),
        }
    }
}

/// Collect every schema gap across the given routes, without generating the
/// spec or emitting any log. This is the testable seam behind the boot-time
/// warnings emitted by [`build_spec`].
///
/// A route is flagged when:
/// - a successful (non-204) response has an unmappable body type
///   ([`SchemaGap::MissingResponseBody`]); the `#[routes]` macro records the
///   offending return type in `RouteInfo.response_unmapped`;
/// - a named response type has no `JsonSchema`
///   ([`SchemaGap::SchemalessResponseBody`]);
/// - a named request type has no `JsonSchema`
///   ([`SchemaGap::SchemalessRequestBody`]).
pub fn spec_warnings(routes: &[RouteInfo]) -> Vec<SpecWarning> {
    let mut warnings = Vec::new();
    for route in routes {
        // Missing response body: no mappable type, and not an intentional
        // no-body 204.
        if route.response_status != 204 && route.response_type.is_none() {
            if let Some(type_name) = &route.response_unmapped {
                warnings.push(SpecWarning {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    gap: SchemaGap::MissingResponseBody {
                        type_name: type_name.clone(),
                    },
                });
            }
        }

        // Named response type present but no schema → generic object.
        if let Some(type_name) = &route.response_type {
            if route.response_schema.is_none() {
                warnings.push(SpecWarning {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    gap: SchemaGap::SchemalessResponseBody {
                        type_name: type_name.clone(),
                    },
                });
            }
        }

        // Named request type present but no schema → generic object. Raw
        // multipart bodies carry a content type but no named type, so they are
        // not flagged.
        if let Some(type_name) = &route.request_body_type {
            if route.request_body_schema.is_none() {
                warnings.push(SpecWarning {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    gap: SchemaGap::SchemalessRequestBody {
                        type_name: type_name.clone(),
                    },
                });
            }
        }
    }
    warnings
}

/// Configuration for the generated OpenAPI specification.
pub struct OpenApiConfig {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub docs_ui: bool,
    pub(crate) schema_registry: SchemaRegistry,
    pub(crate) schema_overrides: HashMap<String, Value>,
    /// The application-level error envelope, documented on every route whose
    /// return type carries no envelope of its own. `None` = `HttpError`.
    pub(crate) error_schema: Option<ErrorSchemaInfo>,
}

impl OpenApiConfig {
    pub fn new(title: &str, version: &str) -> Self {
        Self {
            title: title.to_string(),
            version: version.to_string(),
            description: None,
            docs_ui: false,
            schema_registry: SchemaRegistry::new(),
            schema_overrides: HashMap::new(),
            error_schema: None,
        }
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    pub fn with_docs_ui(mut self, enabled: bool) -> Self {
        self.docs_ui = enabled;
        self
    }

    /// Add a schema for a type implementing `schemars::JsonSchema`.
    ///
    /// The schema will appear in `components/schemas` even if the type is not
    /// referenced by any route.
    pub fn with_schema<T: schemars::JsonSchema>(mut self) -> Self {
        self.schema_registry.register_for::<T>();
        self
    }

    /// Add a manually-crafted schema under the given name.
    pub fn with_raw_schema(mut self, name: &str, schema: Value) -> Self {
        self.schema_registry.register(name, schema);
        self
    }

    /// Merge all schemas from a populated `SchemaRegistry`.
    pub fn with_schema_registry(mut self, registry: SchemaRegistry) -> Self {
        for (name, schema) in registry.into_schemas() {
            self.schema_registry.register(&name, schema);
        }
        self
    }

    /// Override the auto-generated schema for a type.
    ///
    /// This takes precedence over both route-derived and registry schemas,
    /// including the error envelope components (`ErrorResponse`, …).
    pub fn with_schema_override(mut self, name: &str, schema: Value) -> Self {
        self.schema_overrides.insert(name.to_string(), schema);
        self
    }

    /// Document error responses with the envelope `E` — the type passed to
    /// `AppBuilder::error_projection::<E>()`. Routes whose handler returns
    /// `Result<T, E2>` with an envelope `E2` keep documenting `E2`.
    ///
    /// The [`OpenApiPlugin`](crate::OpenApiPlugin) sets this from the
    /// application's `ErrorProjector` bean automatically; this method is for
    /// callers of [`build_spec`] / [`openapi_routes`](crate::openapi_routes).
    pub fn with_error_schema<E: ErrorSchema + ?Sized>(mut self) -> Self {
        self.error_schema = Some(ErrorSchemaInfo::of::<E>());
        self
    }

    /// Same as [`with_error_schema`](Self::with_error_schema), from an
    /// already-captured [`ErrorSchemaInfo`].
    pub fn with_error_schema_info(mut self, info: ErrorSchemaInfo) -> Self {
        self.error_schema = Some(info);
        self
    }

    /// The envelope documented on `route`: the route's own (from its return
    /// type), else the application's, else `HttpError`.
    fn error_schema_for(&self, route: &RouteInfo) -> ErrorSchemaInfo {
        route.error_schema.unwrap_or_else(|| self.app_error_schema())
    }

    /// The application-level envelope (the `ErrorProjector`'s), else
    /// `HttpError` — what the catch-panic layer renders with.
    fn app_error_schema(&self) -> ErrorSchemaInfo {
        self.error_schema
            .unwrap_or_else(ErrorSchemaInfo::of::<HttpError>)
    }
}

/// One documented error body: its component name and its schema, already
/// through [`promote_defs`] (so two bodies compare by what they document).
#[derive(Clone, PartialEq)]
struct ErrorBody {
    name: String,
    schema: Value,
    /// Recorded from the route's own envelope (a `Result<T, E>` return type)
    /// rather than the application's — takes the component slot first.
    declared: bool,
}

/// The error responses of one route: `status → distinct bodies`, from the
/// envelope's `status_of` over the inferred rejection kinds plus its
/// `extra_statuses`. A status with no body maps to an empty list.
///
/// Bodies are deduplicated **by schema**: the same body met under two names
/// (or twice under one) is documented once, under the first-recorded name —
/// the route's envelope is recorded before the application's panic body.
/// Distinct bodies on one status (e.g. the default envelope's
/// `ValidationErrorResponse` and `ErrorResponse` on 400) document as an
/// `anyOf`: a validation body is also a valid plain error body, so `oneOf`
/// would reject it.
///
/// **Panics are documented through `app`, not the route's envelope.** The
/// catch-panic layer renders with the application's `ErrorProjector` whatever
/// the route returns, so every route also gets `app`'s `Internal` response.
/// When the route's envelope is the app's, that is the same status and body
/// (deduplicated); when it differs, both are listed — a different status
/// (e.g. the route remaps `Internal` to 503) as its own response, the same
/// status as an `anyOf`.
///
/// `Internal` stays in every route's inferred kinds: the route's envelope also
/// renders the failures the macro cannot enumerate (`#[managed]` acquire /
/// finalize, `#[inject(request)]` extraction, a route/extractor mismatch).
///
/// Each body's `$defs` are pushed to `error_defs` (merged into
/// `components/schemas` after the route and registry schemas).
fn error_responses(
    schema: &ErrorSchemaInfo,
    app: &ErrorSchemaInfo,
    route: &RouteInfo,
    error_defs: &mut Vec<(String, Value)>,
) -> BTreeMap<u16, Vec<ErrorBody>> {
    let declared = route.error_schema.is_some();
    let mut by_status: BTreeMap<u16, Vec<ErrorBody>> = BTreeMap::new();
    let mut record = |status: u16, body: Option<(String, Value)>, declared: bool| {
        let bodies = by_status.entry(status).or_default();
        if let Some((name, raw)) = body {
            let schema = promote_defs(raw, error_defs);
            if !bodies.iter().any(|b| b.schema == schema) {
                bodies.push(ErrorBody { name, schema, declared });
            }
        }
    };
    for kind in &route.rejection_kinds {
        record(schema.status_of(*kind).as_u16(), schema.body_schema_for(*kind), declared);
    }
    for (status, _) in schema.extra_statuses() {
        record(status.as_u16(), schema.body_schema(), declared);
    }
    // The catch-panic 500, rendered by the application projector.
    record(
        app.status_of(RejectionKind::Internal).as_u16(),
        app.body_schema_for(RejectionKind::Internal),
        false,
    );
    by_status
}

/// The OpenAPI response object for an error status with the given bodies
/// (none = description only). A body whose component slot holds its schema
/// is a `$ref`; one whose name is taken by a different schema is inlined.
fn error_response_object(status: u16, bodies: &[ErrorBody], slots: &Map<String, Value>) -> Value {
    let description = r2e_core::http::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("Error");
    let one = |b: &ErrorBody| {
        if slots.get(&b.name) == Some(&b.schema) {
            json!({ "$ref": format!("#/components/schemas/{}", b.name) })
        } else {
            b.schema.clone()
        }
    };
    let schema = match bodies {
        [] => return json!({ "description": description }),
        [b] => one(b),
        many => json!({ "anyOf": many.iter().map(one).collect::<Vec<_>>() }),
    };
    json!({
        "description": description,
        "content": { "application/json": { "schema": schema } }
    })
}

/// Build an OpenAPI 3.1.0 JSON spec from config and route metadata.
///
/// Every [`SpecWarning`] (see [`build_spec_with_warnings`]) is logged once
/// through `tracing::warn!`.
pub fn build_spec(config: &OpenApiConfig, routes: &[RouteInfo]) -> Value {
    let (spec, warnings) = build_spec_with_warnings(config, routes);
    // Surface schema gaps once, at boot (build_spec runs during plugin install),
    // so silently-undocumented bodies become visible instead of vanishing.
    for warning in warnings {
        // Render the message eagerly (once per gap, at boot) so the warning path
        // is observable without relying on a subscriber-gated lazy format arg.
        let message = warning.message();
        tracing::warn!(
            method = %warning.method,
            path = %warning.path,
            "{message}"
        );
    }
    spec
}

/// [`build_spec`] without logging: the spec plus every warning it would log —
/// the route gaps of [`spec_warnings`] and the error bodies documented inline
/// because their component name is taken ([`SchemaGap::ErrorBodyInlined`]).
pub fn build_spec_with_warnings(
    config: &OpenApiConfig,
    routes: &[RouteInfo],
) -> (Value, Vec<SpecWarning>) {
    let mut warnings = spec_warnings(routes);

    // ── Error bodies, per route (aligned with `routes`) ────────────────────
    let app_schema = config.app_error_schema();
    let mut error_defs: Vec<(String, Value)> = Vec::new();
    let route_errors: Vec<BTreeMap<u16, Vec<ErrorBody>>> = routes
        .iter()
        .map(|route| {
            error_responses(&config.error_schema_for(route), &app_schema, route, &mut error_defs)
        })
        .collect();

    // ── components/schemas ──────────────────────────────────────────────────
    // Collect all referenced types (request body + response) into components/schemas.
    // If the route carries a schemars-generated schema, use it;
    // otherwise fall back to a generic object.
    //
    // schemars 1.x generates JSON Schema Draft 2020-12 (aligned with OpenAPI 3.1.0).
    // We strip `$schema`, promote `$defs` entries to components/schemas,
    // and rewrite `$ref` paths from `#/$defs/X` to `#/components/schemas/X`.
    let mut schemas: Map<String, Value> = Map::new();
    let mut extra_definitions: Vec<(String, Value)> = Vec::new();

    for route in routes {
        // Collect request body schemas
        if let Some(ref body_type) = route.request_body_type {
            if !schemas.contains_key(body_type) {
                insert_schema(
                    &mut schemas,
                    &mut extra_definitions,
                    body_type,
                    &route.request_body_schema,
                );
            }
        }

        // Collect response schemas
        if let Some(ref resp_type) = route.response_type {
            if !schemas.contains_key(resp_type) {
                insert_schema(
                    &mut schemas,
                    &mut extra_definitions,
                    resp_type,
                    &route.response_schema,
                );
            }
        }
    }

    // Merge extra schemas from registry (route schemas take precedence).
    for (name, schema) in config.schema_registry.iter() {
        if !schemas.contains_key(name) {
            insert_schema(
                &mut schemas,
                &mut extra_definitions,
                name,
                &Some(schema.clone()),
            );
        }
    }

    // Merge promoted $defs of the route and registry schemas.
    for (def_name, mut def_schema) in extra_definitions {
        sanitize_schema(&mut def_schema);
        schemas.entry(def_name).or_insert(def_schema);
    }

    // Error envelope components fill the slots left free, the routes' own
    // envelopes before the application's. A body whose name is already taken
    // by a different schema (a DTO, a registry entry, another envelope's
    // body) is inlined in its responses — see `error_response_object`.
    for declared in [true, false] {
        for body in route_errors.iter().flat_map(|r| r.values().flatten()) {
            if body.declared == declared {
                schemas
                    .entry(body.name.clone())
                    .or_insert_with(|| body.schema.clone());
            }
        }
    }
    for (def_name, mut def_schema) in error_defs {
        sanitize_schema(&mut def_schema);
        schemas.entry(def_name).or_insert(def_schema);
    }

    // The slots error references resolve against: explicit overrides (below)
    // are deliberate and never turn a `$ref` into an inline schema.
    let slots = schemas.clone();

    // Apply explicit overrides (replace any existing schema).
    for (name, schema) in &config.schema_overrides {
        let mut s = schema.clone();
        sanitize_schema(&mut s);
        schemas.insert(name.clone(), s);
    }

    // ── paths ───────────────────────────────────────────────────────────────
    let mut paths: Map<String, Value> = Map::new();

    for (route, errors) in routes.iter().zip(&route_errors) {
        let axum_path = route.path.replace('{', "{").replace('}', "}");
        let method_lower = route.method.to_lowercase();

        let mut operation: Map<String, Value> = Map::new();
        operation.insert("operationId".into(), json!(route.operation_id));

        if let Some(ref tag) = route.tag {
            operation.insert("tags".into(), json!([tag]));
        }

        if let Some(ref summary) = route.summary {
            operation.insert("summary".into(), json!(summary));
        }

        // Parameters
        let params: Vec<Value> = route
            .params
            .iter()
            .map(|p| {
                let location = match p.location {
                    ParamLocation::Path => "path",
                    ParamLocation::Query => "query",
                    ParamLocation::Header => "header",
                };
                json!({
                    "name": p.name,
                    "in": location,
                    "required": p.required,
                    "schema": { "type": p.param_type }
                })
            })
            .collect();

        if !params.is_empty() {
            operation.insert("parameters".into(), json!(params));
        }

        // Description
        if let Some(ref description) = route.description {
            operation.insert("description".into(), json!(description));
        }

        // Deprecated
        if route.deprecated {
            operation.insert("deprecated".into(), json!(true));
        }

        // Request body. The media type defaults to application/json; multipart
        // routes carry an explicit request_body_content_type. A content type
        // without a named body type (raw Multipart) is modeled as a free-form
        // object.
        let body_schema = match (&route.request_body_type, &route.request_body_content_type) {
            (Some(body_type), _) => {
                Some(json!({ "$ref": format!("#/components/schemas/{body_type}") }))
            }
            (None, Some(_)) => Some(json!({ "type": "object" })),
            (None, None) => None,
        };
        if let Some(schema) = body_schema {
            let content_type = route
                .request_body_content_type
                .as_deref()
                .unwrap_or("application/json");
            operation.insert(
                "requestBody".into(),
                json!({
                    "required": route.request_body_required,
                    "content": { content_type: { "schema": schema } }
                }),
            );
        }

        // Responses
        let status_key = route.response_status.to_string();
        let status_desc = match route.response_status {
            201 => "Created",
            204 => "No content",
            _ => "Successful response",
        };
        let mut responses: Map<String, Value> = Map::new();

        if route.response_status == 204 {
            // 204 No Content — no response body
            responses.insert(status_key, json!({ "description": status_desc }));
        } else if let Some(ref resp_type) = route.response_type {
            responses.insert(
                status_key,
                json!({
                    "description": status_desc,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": format!("#/components/schemas/{resp_type}") }
                        }
                    }
                }),
            );
        } else {
            responses.insert(status_key, json!({ "description": status_desc }));
        }

        // Error responses: one per distinct status the route's envelope
        // projects its inferred rejection kinds to (+ `extra_statuses`).
        // The success status wins when a kind collides with it.
        let mut inlined: Vec<&str> = Vec::new();
        for (status, bodies) in errors {
            if responses.contains_key(&status.to_string()) {
                continue;
            }
            for b in bodies {
                if slots.get(&b.name) != Some(&b.schema) && !inlined.contains(&b.name.as_str()) {
                    inlined.push(&b.name);
                }
            }
            responses.insert(status.to_string(), error_response_object(*status, bodies, &slots));
        }
        for component in inlined {
            warnings.push(SpecWarning {
                method: route.method.clone(),
                path: route.path.clone(),
                gap: SchemaGap::ErrorBodyInlined {
                    component: component.to_string(),
                },
            });
        }

        operation.insert("responses".into(), Value::Object(responses));

        // Security
        if !route.roles.is_empty() {
            operation.insert("security".into(), json!([{ "bearerAuth": route.roles }]));
        }

        let path_entry = paths.entry(axum_path).or_insert_with(|| json!({}));

        if let Some(obj) = path_entry.as_object_mut() {
            obj.insert(method_lower, Value::Object(operation));
        }
    }

    let mut info: Map<String, Value> = Map::new();
    info.insert("title".into(), json!(config.title));
    info.insert("version".into(), json!(config.version));
    if let Some(ref desc) = config.description {
        info.insert("description".into(), json!(desc));
    }

    let mut components: Map<String, Value> = Map::new();
    components.insert(
        "securitySchemes".into(),
        json!({
            "bearerAuth": {
                "type": "http",
                "scheme": "bearer",
                "bearerFormat": "JWT"
            }
        }),
    );
    if !schemas.is_empty() {
        components.insert("schemas".into(), Value::Object(schemas));
    }

    let spec = json!({
        "openapi": "3.1.0",
        "info": info,
        "paths": paths,
        "components": components
    });
    (spec, warnings)
}
