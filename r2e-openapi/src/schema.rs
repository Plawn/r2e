use serde_json::{Map, Value};
use std::collections::HashMap;

/// Registry that collects JSON Schema definitions for OpenAPI components.
///
/// Types that implement `SchemaProvider` can register themselves here.
/// The registry is then merged into the OpenAPI spec's `components/schemas`.
pub struct SchemaRegistry {
    schemas: HashMap<String, Value>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Register a schema definition under the given name.
    pub fn register(&mut self, name: &str, schema: Value) {
        self.schemas.insert(name.to_string(), schema);
    }

    /// Register a simple object schema with the given fields.
    ///
    /// Each field is `(name, type_string)` where type_string is an OpenAPI
    /// type like `"string"`, `"integer"`, `"number"`, `"boolean"`, `"array"`.
    pub fn register_object(&mut self, name: &str, fields: &[(&str, &str)]) {
        let mut properties = Map::new();
        let mut required = Vec::new();

        for (field_name, field_type) in fields {
            properties.insert(
                field_name.to_string(),
                serde_json::json!({ "type": field_type }),
            );
            required.push(serde_json::json!(field_name));
        }

        let schema = serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        });

        self.schemas.insert(name.to_string(), schema);
    }

    /// Consume the registry and return the schemas map for embedding
    /// in the OpenAPI spec.
    pub fn into_schemas(self) -> HashMap<String, Value> {
        self.schemas
    }

    /// Check if a schema is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.schemas.contains_key(name)
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for types that can provide their own JSON Schema.
///
/// Implement this for your request/response types to enable automatic
/// schema registration in the OpenAPI spec.
pub trait SchemaProvider {
    /// The schema name (typically the type name, e.g. `"User"`).
    fn schema_name() -> &'static str;

    /// Return a JSON Schema representation of this type.
    fn json_schema() -> Value;

    /// Register this type's schema in the given registry.
    fn register_schema(registry: &mut SchemaRegistry) {
        registry.register(Self::schema_name(), Self::json_schema());
    }
}
