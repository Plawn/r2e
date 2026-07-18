use r2e_openapi::schema::{SchemaProvider, SchemaRegistry};
use serde_json::{json, Value};
use std::borrow::Cow;

// ── Phase 1: SchemaRegistry ─────────────────────────────────────────────────

#[test]
fn registry_new_empty() {
    let registry = SchemaRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    let schemas = registry.into_schemas();
    assert!(schemas.is_empty());
}

#[test]
fn register_single_schema() {
    let mut registry = SchemaRegistry::new();
    registry.register("User", json!({"type": "object"}));

    assert!(registry.contains("User"));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
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
    assert!(registry.is_empty());
    assert!(registry.into_schemas().is_empty());
}

#[test]
fn iter_yields_all_entries() {
    let mut registry = SchemaRegistry::new();
    registry.register("A", json!({"type": "string"}));
    registry.register("B", json!({"type": "integer"}));

    let items: Vec<_> = registry.iter().collect();
    assert_eq!(items.len(), 2);
}

// ── Phase 2: SchemaProvider trait ───────────────────────────────────────────

struct TestUser;

impl SchemaProvider for TestUser {
    fn schema_name() -> Cow<'static, str> {
        "TestUser".into()
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

#[test]
fn register_provider_populates_registry() {
    let mut registry = SchemaRegistry::new();
    registry.register_provider::<TestUser>();

    assert!(registry.contains("TestUser"));
    let schemas = registry.into_schemas();
    assert_eq!(schemas["TestUser"]["type"], "object");
}

// ── Phase 3: register_for (schemars integration) ───────────────────────────

#[derive(schemars::JsonSchema)]
struct Widget {
    name: String,
    count: u32,
}

#[test]
fn register_for_json_schema_type() {
    let mut registry = SchemaRegistry::new();
    registry.register_for::<Widget>();

    assert!(registry.contains("Widget"));
    let schemas = registry.into_schemas();
    let widget = &schemas["Widget"];
    assert!(widget["properties"]["name"].is_object());
    assert!(widget["properties"]["count"].is_object());
}

#[derive(schemars::JsonSchema)]
struct Parent {
    child: Child,
}

#[derive(schemars::JsonSchema)]
struct Child {
    value: String,
}

#[test]
fn register_for_nested_type_includes_defs() {
    let mut registry = SchemaRegistry::new();
    registry.register_for::<Parent>();

    // The raw schema should contain $defs for Child.
    // Actual promotion happens in build_spec, but the schema should be stored.
    let schemas = registry.into_schemas();
    let parent = &schemas["Parent"];
    // schemars puts nested types in $defs
    assert!(
        parent.get("$defs").is_some() || parent["properties"]["child"].get("$ref").is_some(),
        "nested type should produce $defs or $ref"
    );
}
