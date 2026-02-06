use r2e_core::openapi::{ParamLocation, RouteInfo};
use serde_json::{json, Map, Value};

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
    let mut schemas: Map<String, Value> = Map::new();
    for route in routes {
        if let Some(ref body_type) = route.request_body_type {
            schemas.entry(body_type.clone()).or_insert_with(|| {
                if let Some(ref root_schema) = route.request_body_schema {
                    // schemars RootSchema — strip meta keys that are invalid in OpenAPI
                    let mut schema = root_schema.clone();
                    if let Some(obj) = schema.as_object_mut() {
                        obj.remove("$schema");
                        obj.remove("definitions");
                    }
                    schema
                } else {
                    json!({
                        "type": "object",
                        "additionalProperties": true
                    })
                }
            });
        }
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
