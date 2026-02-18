use r2e_openapi::schema::{SchemaProvider, SchemaRegistry};
use serde_json::{json, Value};

// ── Phase 1: SchemaRegistry ─────────────────────────────────────────────────

#[test]
fn registry_new_empty() {
    let registry = SchemaRegistry::new();
    let schemas = registry.into_schemas();
    assert!(schemas.is_empty());
}

#[test]
fn register_single_schema() {
    let mut registry = SchemaRegistry::new();
    registry.register("User", json!({"type": "object"}));

    assert!(registry.contains("User"));
    let schemas = registry.into_schemas();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas["User"], json!({"type": "object"}));
}

#[test]
fn register_object_schema() {
    let mut registry = SchemaRegistry::new();
    registry.register_object("User", &[("name", "string"), ("age", "integer")]);

    assert!(registry.contains("User"));
    let schemas = registry.into_schemas();
    let user = &schemas["User"];

    assert_eq!(user["type"], "object");
    assert_eq!(user["properties"]["name"]["type"], "string");
    assert_eq!(user["properties"]["age"]["type"], "integer");
    assert_eq!(user["required"], json!(["name", "age"]));
}

#[test]
fn register_duplicate_overwrites() {
    let mut registry = SchemaRegistry::new();
    registry.register("User", json!({"type": "object", "description": "v1"}));
    registry.register("User", json!({"type": "object", "description": "v2"}));

    let schemas = registry.into_schemas();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas["User"]["description"], "v2");
}

#[test]
fn contains_registered() {
    let mut registry = SchemaRegistry::new();
    registry.register("User", json!({"type": "object"}));
    assert!(registry.contains("User"));
}

#[test]
fn contains_unregistered() {
    let registry = SchemaRegistry::new();
    assert!(!registry.contains("Unknown"));
}

#[test]
fn into_schemas_output() {
    let mut registry = SchemaRegistry::new();
    registry.register("User", json!({"type": "object"}));
    registry.register("Role", json!({"type": "string", "enum": ["admin", "user"]}));

    let schemas = registry.into_schemas();
    assert_eq!(schemas.len(), 2);
    assert!(schemas.contains_key("User"));
    assert!(schemas.contains_key("Role"));
}

#[test]
fn default_creates_empty_registry() {
    let registry = SchemaRegistry::default();
    assert!(!registry.contains("anything"));
    assert!(registry.into_schemas().is_empty());
}

// ── Phase 2: SchemaProvider trait ───────────────────────────────────────────

struct TestUser;

impl SchemaProvider for TestUser {
    fn schema_name() -> &'static str {
        "TestUser"
    }

    fn json_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "name": { "type": "string" },
                "email": { "type": "string" }
            },
            "required": ["id", "name", "email"]
        })
    }
}

#[test]
fn schema_name_returns_type_name() {
    assert_eq!(TestUser::schema_name(), "TestUser");
}

#[test]
fn json_schema_valid_structure() {
    let schema = TestUser::json_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].is_object());
    assert!(schema["required"].is_array());
}

#[test]
fn register_schema_populates_registry() {
    let mut registry = SchemaRegistry::new();
    TestUser::register_schema(&mut registry);

    assert!(registry.contains("TestUser"));
    let schemas = registry.into_schemas();
    assert_eq!(schemas["TestUser"]["type"], "object");
    assert_eq!(schemas["TestUser"]["properties"]["id"]["type"], "integer");
}
