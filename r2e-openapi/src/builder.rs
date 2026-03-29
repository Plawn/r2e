use r2e_core::meta::{ParamLocation, RouteInfo};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

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

/// Insert a schema into the schemas map, promoting `$defs` to top-level components.
fn insert_schema(
    schemas: &mut Map<String, Value>,
    extra_definitions: &mut Vec<(String, Value)>,
    type_name: &str,
    root_schema: &Option<Value>,
) {
    if let Some(ref root) = root_schema {
        let mut schema = root.clone();
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
        schemas.insert(type_name.to_string(), schema);
    } else {
        schemas.insert(type_name.to_string(), json!({ "type": "object" }));
    }
}

/// Configuration for the generated OpenAPI specification.
pub struct OpenApiConfig {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub docs_ui: bool,
    pub(crate) schema_registry: SchemaRegistry,
    pub(crate) schema_overrides: HashMap<String, Value>,
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
    /// This takes precedence over both route-derived and registry schemas.
    pub fn with_schema_override(mut self, name: &str, schema: Value) -> Self {
        self.schema_overrides.insert(name.to_string(), schema);
        self
    }
}

/// Build an OpenAPI 3.1.0 JSON spec from config and route metadata.
pub fn build_spec(config: &OpenApiConfig, routes: &[RouteInfo]) -> Value {
    let mut paths: Map<String, Value> = Map::new();

    for route in routes {
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

        // Request body
        if let Some(ref body_type) = route.request_body_type {
            operation.insert(
                "requestBody".into(),
                json!({
                    "required": route.request_body_required,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": format!("#/components/schemas/{body_type}") }
                        }
                    }
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

        // Conditional 401/403 only when route has auth
        if route.has_auth {
            responses.insert("401".into(), json!({
                "description": "Unauthorized",
                "content": {
                    "application/json": {
                        "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                    }
                }
            }));
            responses.insert("403".into(), json!({
                "description": "Forbidden",
                "content": {
                    "application/json": {
                        "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                    }
                }
            }));
        }

        // Default 500 response
        responses.insert("500".into(), json!({
            "description": "Internal server error",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                }
            }
        }));

        // If route has a request body, it may return 400
        if route.request_body_type.is_some() {
            responses.entry("400".to_string()).or_insert_with(|| json!({
                "description": "Bad request / Validation error",
                "content": {
                    "application/json": {
                        "schema": { "$ref": "#/components/schemas/ValidationErrorResponse" }
                    }
                }
            }));
        }

        operation.insert("responses".into(), Value::Object(responses));

        // Security
        if !route.roles.is_empty() {
            operation.insert(
                "security".into(),
                json!([{ "bearerAuth": route.roles }]),
            );
        }

        let path_entry = paths
            .entry(axum_path)
            .or_insert_with(|| json!({}));

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
                insert_schema(&mut schemas, &mut extra_definitions, body_type, &route.request_body_schema);
            }
        }

        // Collect response schemas
        if let Some(ref resp_type) = route.response_type {
            if !schemas.contains_key(resp_type) {
                insert_schema(&mut schemas, &mut extra_definitions, resp_type, &route.response_schema);
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

    // Merge promoted $defs from all sources (routes + registry).
    for (def_name, mut def_schema) in extra_definitions {
        sanitize_schema(&mut def_schema);
        schemas.entry(def_name).or_insert(def_schema);
    }

    // Apply explicit overrides (replace any existing schema).
    for (name, schema) in &config.schema_overrides {
        let mut s = schema.clone();
        sanitize_schema(&mut s);
        schemas.insert(name.clone(), s);
    }

    // Insert standard error schemas
    schemas.entry("ErrorResponse".to_string()).or_insert_with(|| {
        json!({
            "type": "object",
            "properties": {
                "error": {
                    "type": "string",
                    "description": "A human-readable error message"
                }
            },
            "required": ["error"]
        })
    });
    schemas.entry("ValidationErrorResponse".to_string()).or_insert_with(|| {
        json!({
            "type": "object",
            "properties": {
                "error": {
                    "type": "string",
                    "description": "Always \"Validation failed\""
                },
                "details": {
                    "type": "array",
                    "items": {
                        "$ref": "#/components/schemas/FieldError"
                    }
                }
            },
            "required": ["error", "details"]
        })
    });
    schemas.entry("FieldError".to_string()).or_insert_with(|| {
        json!({
            "type": "object",
            "properties": {
                "field": { "type": "string" },
                "message": { "type": "string" },
                "code": { "type": "string" }
            },
            "required": ["field", "message", "code"]
        })
    });

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

    json!({
        "openapi": "3.1.0",
        "info": info,
        "paths": paths,
        "components": components
    })
}
