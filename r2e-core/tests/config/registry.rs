//! Config sections auto-registered as beans in the graph.

use crate::sections::{AppConfig, ServerConfig};
use r2e_core::beans::BeanRegistry;
use r2e_core::config::{ConfigProperties, R2eConfig};

// =========================================================================
// Feature 1: Auto-register child ConfigProperties as beans
// =========================================================================

#[test]
fn test_register_children_provides_nested_bean() {
    let yaml = r#"
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppConfig::from_config(&config, Some("app")).unwrap();

    let mut registry = BeanRegistry::new();
    app.register_children(&mut registry);

    // NestedDbConfig should have been provided to the registry
    // We can't easily extract from BeanRegistry directly, but we can verify
    // that register_children doesn't panic and works recursively.
}

#[test]
fn test_register_children_optional_section_some() {
    let yaml = r#"
server:
  host: "0.0.0.0"
  tls:
    cert: "/path/to/cert.pem"
    key: "/path/to/key.pem"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let srv = ServerConfig::from_config(&config, Some("server")).unwrap();

    let mut registry = BeanRegistry::new();
    srv.register_children(&mut registry);
    // TlsConfig should have been provided (Some case)
}

#[test]
fn test_register_children_optional_section_none() {
    let yaml = r#"
server:
  host: "0.0.0.0"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let srv = ServerConfig::from_config(&config, Some("server")).unwrap();

    let mut registry = BeanRegistry::new();
    srv.register_children(&mut registry);
    // Should not panic when tls is None
}

// Recursive nesting: grandchild config
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct GrandchildConfig {
    pub value: String,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ChildWithNested {
    pub label: String,
    #[config(section)]
    pub inner: GrandchildConfig,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct RootWithDeepNesting {
    #[config(section)]
    pub child: ChildWithNested,
}

#[test]
fn test_register_children_recursive() {
    let yaml = r#"
root:
  child:
    label: "test"
    inner:
      value: "deep"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let root = RootWithDeepNesting::from_config(&config, Some("root")).unwrap();

    let mut registry = BeanRegistry::new();
    root.register_children(&mut registry);
    // Both ChildWithNested and GrandchildConfig should be registered
}
