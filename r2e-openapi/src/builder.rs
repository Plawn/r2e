use r2e_core::openapi::{ParamLocation, RouteInfo};
use serde_json::{json, Map, Value};

/// Recursively sanitize a JSON Schema value for OpenAPI 3.0.3 compatibility.
///
/// OpenAPI UI tools (wti-element, Swagger UI, etc.) may not handle boolean schemas
/// from JSON Schema Draft 7. This function:
/// - Replaces `"additionalProperties": true` → removes the key (true is the default)
/// - Replaces `"additionalProperties": false` → `"additionalProperties": {}`-equivalent (kept as false, most tools handle it)
/// - Rewrites `$ref` paths from `#/definitions/X` to `#/components/schemas/X`
fn sanitize_schema(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            // Rewrite $ref from schemars format to OpenAPI format
            if let Some(Value::String(ref_str)) = obj.get_mut("$ref") {
                if ref_str.starts_with("#/definitions/") {
                    *ref_str = ref_str.replace("#/definitions/", "#/components/schemas/");
                }
            }

            // Replace boolean additionalProperties with schema objects
            if let Some(ap) = obj.get("additionalProperties") {
                if ap.as_bool() == Some(true) {
                    obj.remove("additionalProperties");
                }
                // Note: `false` is kept as-is since most tools handle it correctly
            }

            // Recurse into all object values
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

/// Configuration for the generated OpenAPI specification.
pub struct OpenApiConfig {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub docs_ui: bool,
}

impl OpenApiConfig {
    pub fn new(title: &str, version: &str) -> Self {
        Self {
            title: title.to_string(),
            version: version.to_string(),
            description: None,
            docs_ui: false,
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
}

/// Build an OpenAPI 3.0 JSON spec from config and route metadata.
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

        // Request body
        if let Some(ref body_type) = route.request_body_type {
            operation.insert(
                "requestBody".into(),
                json!({
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": format!("#/components/schemas/{body_type}") }
                        }
                    }
                }),
            );
        }

        // Responses
        operation.insert(
            "responses".into(),
            json!({
                "200": {
                    "description": "Successful response"
                },
                "401": {
                    "description": "Unauthorized"
                },
                "403": {
                    "description": "Forbidden"
                }
            }),
        );

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

    // Collect all referenced body types into components/schemas.
    // If the route carries a schemars-generated schema, use it;
    // otherwise fall back to a generic object.
    //
    // schemars generates JSON Schema Draft 7 which needs adaptation for OpenAPI 3.0.3:
    // - `$schema` and `definitions` keys are stripped from the root
    // - `definitions` entries are promoted to components/schemas
    // - `$ref` paths are rewritten from `#/definitions/X` to `#/components/schemas/X`
    // - boolean `additionalProperties` values are cleaned up
    let mut schemas: Map<String, Value> = Map::new();
    let mut extra_definitions: Vec<(String, Value)> = Vec::new();

    for route in routes {
        if let Some(ref body_type) = route.request_body_type {
            if schemas.contains_key(body_type) {
                continue;
            }
            if let Some(ref root_schema) = route.request_body_schema {
                let mut schema = root_schema.clone();
                if let Some(obj) = schema.as_object_mut() {
                    obj.remove("$schema");
                    // Extract schemars `definitions` and promote them to
                    // components/schemas so that $ref links resolve correctly.
                    if let Some(Value::Object(defs)) = obj.remove("definitions") {
                        for (def_name, def_schema) in defs {
                            extra_definitions.push((def_name, def_schema));
                        }
                    }
                }
                sanitize_schema(&mut schema);
                schemas.insert(body_type.clone(), schema);
            } else {
                schemas.insert(body_type.clone(), json!({ "type": "object" }));
            }
        }
    }

    // Merge promoted definitions from schemars into components/schemas.
    for (def_name, mut def_schema) in extra_definitions {
        sanitize_schema(&mut def_schema);
        schemas.entry(def_name).or_insert(def_schema);
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

    json!({
        "openapi": "3.0.3",
        "info": info,
        "paths": paths,
        "components": components
    })
}
