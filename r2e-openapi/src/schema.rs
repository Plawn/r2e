use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;

/// Registry that collects JSON Schema definitions for OpenAPI components.
///
/// Schemas registered here are merged into the generated OpenAPI spec's
/// `components/schemas`. Use this to expose types that don't appear in any
/// route (WebSocket messages, domain events, shared DTOs) or to provide
/// manual schemas for external types without a `JsonSchema` derive.
///
/// # Example
///
/// ```ignore
/// use r2e_openapi::OpenApiConfig;
///
/// OpenApiConfig::new("My API", "1.0.0")
///     .with_schema::<WsMessage>()          // from schemars::JsonSchema
///     .with_raw_schema("External", json!({"type": "object"}))
///     .with_docs_ui(true)
/// ```
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

    /// Register a schema derived from a type implementing `schemars::JsonSchema`.
    ///
    /// Generates the JSON Schema via `SchemaGenerator` and registers it under
    /// the type's schema name. Any `$defs` promotion and `$ref` rewriting is
    /// handled later by `build_spec()`.
    pub fn register_for<T: schemars::JsonSchema>(&mut self) {
        let name = T::schema_name().into_owned();
        let schema = schemars::SchemaGenerator::default().into_root_schema_for::<T>();
        self.schemas.insert(name, Value::from(schema));
    }

    /// Register a schema from a type implementing `SchemaProvider`.
    pub fn register_provider<T: SchemaProvider>(&mut self) {
        T::register_schema(self);
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

    /// Iterate over all registered schemas.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.schemas.iter()
    }

    /// Number of registered schemas.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Returns `true` if no schemas are registered.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
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
/// Implement this for types that don't derive `schemars::JsonSchema` but
/// still need to appear in the OpenAPI spec. For types with a `JsonSchema`
/// derive, prefer `SchemaRegistry::register_for::<T>()` instead.
pub trait SchemaProvider {
    /// The schema name (typically the type name, e.g. `"User"`).
    fn schema_name() -> Cow<'static, str>;

    /// Return a JSON Schema representation of this type.
    fn json_schema() -> Value;

    /// Register this type's schema in the given registry.
    fn register_schema(registry: &mut SchemaRegistry) {
        registry.register(&Self::schema_name(), Self::json_schema());
    }
}
