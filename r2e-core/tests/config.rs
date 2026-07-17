use r2e_core::beans::BeanRegistry;
use r2e_core::config::{ConfigError, ConfigProperties, ConfigValue, R2eConfig};

#[test]
fn test_empty_config() {
    let config = R2eConfig::empty();
    assert!(config.get::<String>("nonexistent").is_err());
}

#[test]
fn test_set_and_get() {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("test".into()));
    assert_eq!(config.get::<String>("app.name").unwrap(), "test");
}

#[test]
fn test_get_or_default() {
    let config = R2eConfig::empty();
    assert_eq!(config.get_or("missing", 42i64), 42);
}

#[test]
fn test_get_opt() {
    let mut config = R2eConfig::empty();
    config.set("present", ConfigValue::Integer(7));
    config.set("null_val", ConfigValue::Null);
    config.set("wrong_type", ConfigValue::String("not-an-int".into()));

    // Absent → Ok(None); explicit null → Ok(None); present → Ok(Some(v)).
    assert_eq!(config.get_opt::<i64>("missing").unwrap(), None);
    assert_eq!(config.get_opt::<i64>("null_val").unwrap(), None);
    assert_eq!(config.get_opt::<i64>("present").unwrap(), Some(7));

    // Unlike `try_get` (fail-open → None), a type mismatch is a loud error.
    assert!(config.get_opt::<i64>("wrong_type").is_err());
    assert_eq!(config.try_get::<i64>("wrong_type"), None);
}

#[test]
fn test_type_conversions() {
    let mut config = R2eConfig::empty();
    config.set("int_val", ConfigValue::Integer(42));
    config.set("float_val", ConfigValue::Float(3.14));
    config.set("bool_val", ConfigValue::Bool(true));
    config.set("null_val", ConfigValue::Null);

    assert_eq!(config.get::<i64>("int_val").unwrap(), 42);
    assert_eq!(config.get::<f64>("float_val").unwrap(), 3.14);
    assert!(config.get::<bool>("bool_val").unwrap());
    assert_eq!(config.get::<String>("int_val").unwrap(), "42");
    assert!(config.get::<Option<String>>("null_val").unwrap().is_none());
}

#[test]
fn test_flatten_yaml() {
    let yaml = r#"
app:
  database:
    url: "sqlite::memory:"
    pool_size: 10
  name: "test"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();

    assert_eq!(
        config.get::<String>("app.database.url").unwrap(),
        "sqlite::memory:"
    );
    assert_eq!(config.get::<i64>("app.database.pool_size").unwrap(), 10);
    assert_eq!(config.get::<String>("app.name").unwrap(), "test");
}

#[test]
fn test_list_config() {
    let yaml = r#"
app:
  origins:
    - "http://localhost"
    - "https://prod.com"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let origins: Vec<String> = config.get("app.origins").unwrap();
    assert_eq!(origins, vec!["http://localhost", "https://prod.com"]);
}

#[test]
fn test_list_indexed_access() {
    let yaml = r#"
app:
  features:
    - "openapi"
    - "prometheus"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert_eq!(
        config.get::<String>("app.features.0").unwrap(),
        "openapi"
    );
    assert_eq!(
        config.get::<String>("app.features.1").unwrap(),
        "prometheus"
    );
}

#[test]
fn test_single_value_as_vec() {
    let mut config = R2eConfig::empty();
    config.set("single", ConfigValue::String("only-one".into()));
    let result: Vec<String> = config.get("single").unwrap();
    assert_eq!(result, vec!["only-one"]);
}

#[test]
fn test_contains_key() {
    let mut config = R2eConfig::empty();
    config.set("exists", ConfigValue::String("yes".into()));
    assert!(config.contains_key("exists"));
    assert!(!config.contains_key("nope"));
}

// --- ConfigProperties: basic usage (required, optional, default) ---

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DatabaseConfig {
    pub url: String,
    #[config(default = 10)]
    pub pool_size: i64,
    pub timeout: Option<i64>,
}

#[test]
fn test_config_properties_basic() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let db = DatabaseConfig::from_config(&config, Some("app.database")).unwrap();

    assert_eq!(db.url, "postgres://localhost/mydb");
    assert_eq!(db.pool_size, 10); // default applied
    assert!(db.timeout.is_none()); // optional, absent
}

#[test]
fn test_config_properties_basic_override_default() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
    pool_size: 50
    timeout: 30
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let db = DatabaseConfig::from_config(&config, Some("app.database")).unwrap();

    assert_eq!(db.url, "postgres://localhost/mydb");
    assert_eq!(db.pool_size, 50); // yaml overrides default
    assert_eq!(db.timeout, Some(30));
}

#[test]
fn test_config_properties_basic_missing_required() {
    let config = R2eConfig::empty();
    let result = DatabaseConfig::from_config(&config, Some("app.database"));

    assert!(result.is_err()); // "url" is required
}

// --- ConfigProperties: #[config(key = "...")] custom key mapping ---

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct OidcTestConfig {
    pub issuer: Option<String>,
    #[config(key = "jwks.url")]
    pub jwks_url: Option<String>,
    #[config(default = "my-app")]
    pub audience: String,
    #[config(key = "client.id", default = "my-app")]
    pub client_id: String,
}

#[test]
fn test_config_properties_custom_key() {
    let yaml = r#"
oidc:
  issuer: "https://auth.example.com"
  jwks:
    url: "https://auth.example.com/.well-known/jwks.json"
  client:
    id: "custom-client"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let oidc = OidcTestConfig::from_config(&config, Some("oidc")).unwrap();

    assert_eq!(oidc.issuer.as_deref(), Some("https://auth.example.com"));
    assert_eq!(
        oidc.jwks_url.as_deref(),
        Some("https://auth.example.com/.well-known/jwks.json")
    );
    assert_eq!(oidc.audience, "my-app"); // default, not in yaml
    assert_eq!(oidc.client_id, "custom-client"); // yaml overrides default
}

#[test]
fn test_config_properties_custom_key_defaults() {
    let config = R2eConfig::empty();
    let oidc = OidcTestConfig::from_config(&config, Some("oidc")).unwrap();

    assert!(oidc.issuer.is_none());
    assert!(oidc.jwks_url.is_none());
    assert_eq!(oidc.audience, "my-app");
    assert_eq!(oidc.client_id, "my-app");
}

#[test]
fn test_config_properties_custom_key_metadata() {
    let meta = OidcTestConfig::properties_metadata(Some("oidc"));

    let jwks_meta = meta.iter().find(|m| m.full_key == "oidc.jwks.url").unwrap();
    assert_eq!(jwks_meta.key, "jwks.url");
    assert!(!jwks_meta.required);

    let client_meta = meta.iter().find(|m| m.full_key == "oidc.client.id").unwrap();
    assert_eq!(client_meta.key, "client.id");
    assert!(!client_meta.required); // has default
    assert!(client_meta.default_value.is_some());
}

// =========================================================================
// FromConfigValue — numeric types
// =========================================================================

#[test]
fn test_from_config_value_u16() {
    let mut config = R2eConfig::empty();
    config.set("port", ConfigValue::Integer(8080));
    assert_eq!(config.get::<u16>("port").unwrap(), 8080);
}

#[test]
fn test_from_config_value_u32() {
    let mut config = R2eConfig::empty();
    config.set("count", ConfigValue::Integer(100_000));
    assert_eq!(config.get::<u32>("count").unwrap(), 100_000);
}

#[test]
fn test_from_config_value_u8() {
    let mut config = R2eConfig::empty();
    config.set("level", ConfigValue::Integer(255));
    assert_eq!(config.get::<u8>("level").unwrap(), 255);
}

#[test]
fn test_from_config_value_u8_out_of_range() {
    let mut config = R2eConfig::empty();
    config.set("level", ConfigValue::Integer(256));
    assert!(config.get::<u8>("level").is_err());
}

#[test]
fn test_from_config_value_i32() {
    let mut config = R2eConfig::empty();
    config.set("val", ConfigValue::Integer(-42));
    assert_eq!(config.get::<i32>("val").unwrap(), -42);
}

#[test]
fn test_from_config_value_usize() {
    let mut config = R2eConfig::empty();
    config.set("size", ConfigValue::Integer(1024));
    assert_eq!(config.get::<usize>("size").unwrap(), 1024);
}

#[test]
fn test_from_config_value_f32() {
    let mut config = R2eConfig::empty();
    config.set("ratio", ConfigValue::Float(1.5));
    let val = config.get::<f32>("ratio").unwrap();
    assert!((val - 1.5).abs() < f32::EPSILON);
}

#[test]
fn test_from_config_value_hashmap() {
    use std::collections::HashMap;
    let mut inner = HashMap::new();
    inner.insert("env".to_string(), ConfigValue::String("production".into()));
    inner.insert("region".to_string(), ConfigValue::String("us-east".into()));
    let mut config = R2eConfig::empty();
    config.set("labels", ConfigValue::Map(inner));

    let labels: HashMap<String, String> = config.get("labels").unwrap();
    assert_eq!(labels.get("env").unwrap(), "production");
    assert_eq!(labels.get("region").unwrap(), "us-east");
}

// =========================================================================
// ConfigProperties — #[config(section)] nesting
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct NestedDbConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct AppConfig {
    pub name: String,
    #[config(section)]
    pub database: NestedDbConfig,
}

#[test]
fn test_config_section_nesting() {
    let yaml = r#"
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
    pool_size: 20
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppConfig::from_config(&config, Some("app")).unwrap();

    assert_eq!(app.name, "my-app");
    assert_eq!(app.database.url, "postgres://localhost/mydb");
    assert_eq!(app.database.pool_size, 20);
}

#[test]
fn test_config_section_nesting_defaults() {
    let yaml = r#"
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppConfig::from_config(&config, Some("app")).unwrap();

    assert_eq!(app.database.pool_size, 5); // default from NestedDbConfig
}

#[test]
fn test_config_section_standalone() {
    let yaml = r#"
database:
  url: "sqlite::memory:"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let db = NestedDbConfig::from_config(&config, Some("database")).unwrap();

    assert_eq!(db.url, "sqlite::memory:");
    assert_eq!(db.pool_size, 5);
}

// =========================================================================
// Optional section
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ServerConfig {
    pub host: String,
    #[config(section)]
    pub tls: Option<TlsConfig>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct TlsConfig {
    pub cert: String,
    pub key: String,
}

#[test]
fn test_config_optional_section_present() {
    let yaml = r#"
server:
  host: "0.0.0.0"
  tls:
    cert: "/path/to/cert.pem"
    key: "/path/to/key.pem"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let srv = ServerConfig::from_config(&config, Some("server")).unwrap();

    assert_eq!(srv.host, "0.0.0.0");
    let tls = srv.tls.as_ref().unwrap();
    assert_eq!(tls.cert, "/path/to/cert.pem");
    assert_eq!(tls.key, "/path/to/key.pem");
}

#[test]
fn test_config_optional_section_absent() {
    let yaml = r#"
server:
  host: "0.0.0.0"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let srv = ServerConfig::from_config(&config, Some("server")).unwrap();

    assert_eq!(srv.host, "0.0.0.0");
    assert!(srv.tls.is_none());
}

// =========================================================================
// #[config(env = "...")] — explicit env var override
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DbEnvConfig {
    #[config(env = "TEST_R2E_DATABASE_URL")]
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[test]
fn test_config_env_override() {
    std::env::set_var("TEST_R2E_DATABASE_URL", "postgres://from-env/mydb");

    let config = R2eConfig::empty();
    let db = DbEnvConfig::from_config(&config, Some("db")).unwrap();
    assert_eq!(db.url, "postgres://from-env/mydb");

    std::env::remove_var("TEST_R2E_DATABASE_URL");
}

#[test]
fn test_config_env_override_yaml_takes_priority() {
    std::env::set_var("TEST_R2E_DATABASE_URL", "postgres://from-env/mydb");

    let yaml = r#"
db:
  url: "postgres://from-yaml/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let db = DbEnvConfig::from_config(&config, Some("db")).unwrap();
    assert_eq!(db.url, "postgres://from-yaml/mydb");

    std::env::remove_var("TEST_R2E_DATABASE_URL");
}

// =========================================================================
// ConfigError::Validation
// =========================================================================

#[test]
fn test_config_validation_error_display() {
    use r2e_core::config::ConfigValidationDetail;
    let err = ConfigError::Validation(vec![
        ConfigValidationDetail {
            key: "app.port".to_string(),
            message: "must be between 1 and 65535".to_string(),
        },
    ]);
    let msg = err.to_string();
    assert!(msg.contains("app.port"));
    assert!(msg.contains("must be between 1 and 65535"));
}

// =========================================================================
// PropertyMeta — new fields
// =========================================================================

#[test]
fn test_property_meta_env_var() {
    let meta = DbEnvConfig::properties_metadata(Some("db"));
    let url_meta = meta.iter().find(|m| m.key == "url").unwrap();
    assert_eq!(url_meta.env_var.as_deref(), Some("TEST_R2E_DATABASE_URL"));
}

#[test]
fn test_property_meta_section_flag() {
    let meta = AppConfig::properties_metadata(Some("app"));
    let db_meta = meta.iter().find(|m| m.key == "database").unwrap();
    assert!(db_meta.is_section);
    let name_meta = meta.iter().find(|m| m.key == "name").unwrap();
    assert!(!name_meta.is_section);
}

// =========================================================================
// from_config with custom prefix — runtime prefix override
// =========================================================================

#[test]
fn test_from_config_custom_prefix() {
    let yaml = r#"
custom:
  prefix:
    url: "postgres://custom/mydb"
    pool_size: 42
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let db = NestedDbConfig::from_config(&config, Some("custom.prefix")).unwrap();

    assert_eq!(db.url, "postgres://custom/mydb");
    assert_eq!(db.pool_size, 42);
}

// =========================================================================
// ConfigProperties with u16 field
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct PortConfig {
    #[config(default = 3000)]
    pub port: u16,
    pub host: Option<String>,
}

#[test]
fn test_config_properties_u16() {
    let yaml = r#"
server:
  port: 8080
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let srv = PortConfig::from_config(&config, Some("server")).unwrap();
    assert_eq!(srv.port, 8080);
}

#[test]
fn test_config_properties_u16_default() {
    let config = R2eConfig::empty();
    let srv = PortConfig::from_config(&config, Some("server")).unwrap();
    assert_eq!(srv.port, 3000);
}

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

// =========================================================================
// Feature 2: FromConfigValue derive — enum support via serde
// =========================================================================

#[derive(serde::Deserialize, r2e_macros::FromConfigValue, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum AppMode {
    Development,
    Production,
    Staging,
}

#[test]
fn test_from_config_value_enum() {
    let mut config = R2eConfig::empty();
    config.set("mode", ConfigValue::String("production".into()));
    let mode: AppMode = config.get("mode").unwrap();
    assert_eq!(mode, AppMode::Production);
}

#[test]
fn test_from_config_value_enum_invalid() {
    let mut config = R2eConfig::empty();
    config.set("mode", ConfigValue::String("invalid".into()));
    let result = config.get::<AppMode>("mode");
    assert!(result.is_err());
    match result.unwrap_err() {
        ConfigError::Deserialize { key, message } => {
            assert_eq!(key, "mode");
            assert!(message.contains("unknown variant"));
        }
        other => panic!("Expected Deserialize error, got: {other:?}"),
    }
}

#[test]
fn test_from_config_value_enum_option() {
    let config = R2eConfig::empty();
    let result: Option<AppMode> = config.get_or("mode", None);
    assert!(result.is_none());

    let mut config2 = R2eConfig::empty();
    config2.set("mode", ConfigValue::String("staging".into()));
    let result2: Option<AppMode> = config2.get("mode").unwrap();
    assert_eq!(result2, Some(AppMode::Staging));
}

// Enum as field in ConfigProperties struct
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct AppWithMode {
    pub name: String,
    pub mode: Option<AppMode>,
}

#[test]
fn test_enum_in_config_properties() {
    let yaml = r#"
app:
  name: "my-app"
  mode: "production"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppWithMode::from_config(&config, Some("app")).unwrap();
    assert_eq!(app.name, "my-app");
    assert_eq!(app.mode, Some(AppMode::Production));
}

#[test]
fn test_enum_in_config_properties_absent() {
    let yaml = r#"
app:
  name: "my-app"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppWithMode::from_config(&config, Some("app")).unwrap();
    assert_eq!(app.mode, None);
}

// General case: struct with #[derive(Deserialize, FromConfigValue)]
#[derive(serde::Deserialize, r2e_macros::FromConfigValue, Clone, Debug, PartialEq)]
struct Endpoint {
    pub host: String,
    pub port: u16,
}

#[test]
fn test_from_config_value_struct() {
    use std::collections::HashMap;
    let mut inner = HashMap::new();
    inner.insert("host".to_string(), ConfigValue::String("localhost".into()));
    inner.insert("port".to_string(), ConfigValue::Integer(8080));

    let mut config = R2eConfig::empty();
    config.set("endpoint", ConfigValue::Map(inner));

    let ep: Endpoint = config.get("endpoint").unwrap();
    assert_eq!(ep.host, "localhost");
    assert_eq!(ep.port, 8080);
}

// =========================================================================
// R2E_-prefixed env overlay
// =========================================================================

#[test]
fn test_env_overlay_applies_r2e_prefixed_vars() {
    use std::collections::HashMap;
    let mut values: HashMap<String, ConfigValue> = HashMap::new();
    let env = vec![
        ("R2E_SERVER_PORT".to_string(), "8080".to_string()),
        ("R2E_APP_NAME".to_string(), "svc".to_string()),
    ];
    R2eConfig::apply_env_overlay_for_test(&mut values, env);

    // Build a config around the values map to check via the public API.
    let mut cfg = R2eConfig::empty();
    for (k, v) in values {
        cfg.set(&k, v);
    }
    assert_eq!(cfg.get::<String>("server.port").unwrap(), "8080");
    assert_eq!(cfg.get::<String>("app.name").unwrap(), "svc");
}

#[test]
fn test_env_overlay_ignores_unprefixed_vars() {
    use std::collections::HashMap;
    let mut values: HashMap<String, ConfigValue> = HashMap::new();
    let env = vec![
        ("HOME".to_string(), "/home/me".to_string()),
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("SERVER_PORT".to_string(), "9999".to_string()),
    ];
    R2eConfig::apply_env_overlay_for_test(&mut values, env);
    assert!(values.is_empty(), "non-R2E_ vars must not leak into config");
}

#[test]
fn test_env_overlay_bare_prefix_is_ignored() {
    use std::collections::HashMap;
    let mut values: HashMap<String, ConfigValue> = HashMap::new();
    let env = vec![("R2E_".to_string(), "noop".to_string())];
    R2eConfig::apply_env_overlay_for_test(&mut values, env);
    assert!(values.is_empty());
}

// =========================================================================
// ConfigError::Deserialize display
// =========================================================================

#[test]
fn test_config_deserialize_error_display() {
    let err = ConfigError::Deserialize {
        key: "app.mode".to_string(),
        message: "unknown variant `bad`".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("app.mode"));
    assert!(msg.contains("unknown variant `bad`"));
}

// =========================================================================
// Prefix helpers: has_prefix / sub_keys
// =========================================================================

#[test]
fn test_has_prefix() {
    let yaml = r#"
app:
  database:
    url: "sqlite::memory:"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert!(config.has_prefix("app"));
    assert!(config.has_prefix("app.database"));
    assert!(config.has_prefix("app.database.url")); // exact key counts
    assert!(!config.has_prefix("app.data")); // not a segment boundary
    assert!(!config.has_prefix("missing"));
}

#[test]
fn test_sub_keys() {
    let yaml = r#"
upstreams:
  npm:
    url: "https://registry.npmjs.org"
  docker:
    url: "https://registry-1.docker.io"
    auth:
      token: "x"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert_eq!(config.sub_keys("upstreams"), vec!["docker", "npm"]);
    assert_eq!(config.sub_keys("upstreams.docker"), vec!["auth", "url"]);
    assert!(config.sub_keys("missing").is_empty());
}

// =========================================================================
// Optional sections are presence-based
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct AllDefaultsSection {
    #[config(default = true)]
    enabled: bool,
    #[config(default = 300)]
    ttl_secs: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct RequiredFieldSection {
    url: String,
    #[config(default = 5)]
    pool_size: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct PresenceRoot {
    #[config(section)]
    cache: Option<AllDefaultsSection>,
    #[config(section)]
    database: Option<RequiredFieldSection>,
}

#[test]
fn test_optional_section_absent_is_none_even_with_all_defaults() {
    // Regression: an absent section whose fields all have defaults used to
    // come back as Some(defaults) because from_config never hit NotFound.
    let config = R2eConfig::from_yaml_str("other:\n  x: 1\n").unwrap();
    let root = PresenceRoot::from_config(&config, None).unwrap();
    assert!(root.cache.is_none());
    assert!(root.database.is_none());
}

#[test]
fn test_optional_section_present_parses() {
    let yaml = r#"
cache:
  ttl_secs: 60
database:
  url: "postgres://localhost/db"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let root = PresenceRoot::from_config(&config, None).unwrap();
    let cache = root.cache.unwrap();
    assert!(cache.enabled); // default
    assert_eq!(cache.ttl_secs, 60);
    assert_eq!(root.database.unwrap().url, "postgres://localhost/db");
}

#[test]
fn test_optional_section_partial_errors_instead_of_none() {
    // A present-but-invalid section surfaces the error instead of silently
    // collapsing to None.
    let yaml = r#"
database:
  pool_size: 10
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let err = PresenceRoot::from_config(&config, None).unwrap_err();
    assert!(matches!(err, ConfigError::NotFound(key) if key == "database.url"));
}

// =========================================================================
// #[config(section, default)] — Default fallback for absent sections
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ServerSection {
    #[config(default = "0.0.0.0:8080")]
    bind: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:9999".to_string(),
        }
    }
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DefaultSectionRoot {
    #[config(section, default)]
    server: ServerSection,
}

#[test]
fn test_section_default_used_when_absent() {
    let config = R2eConfig::empty();
    let root = DefaultSectionRoot::from_config(&config, None).unwrap();
    // Default::default(), not the field-level config defaults
    assert_eq!(root.server.bind, "0.0.0.0:9999");
}

#[test]
fn test_section_default_parses_when_present() {
    let config = R2eConfig::from_yaml_str("server:\n  bind: \"127.0.0.1:3000\"\n").unwrap();
    let root = DefaultSectionRoot::from_config(&config, None).unwrap();
    assert_eq!(root.server.bind, "127.0.0.1:3000");
}

// =========================================================================
// #[config(skip)] — field not read from config
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct SkipRoot {
    name: String,
    #[config(skip)]
    resolved_token: Option<String>,
}

#[test]
fn test_skip_field_defaults_and_has_no_metadata() {
    let config = R2eConfig::from_yaml_str("name: app\nresolved_token: leaked\n").unwrap();
    let root = SkipRoot::from_config(&config, None).unwrap();
    assert_eq!(root.name, "app");
    // Never read from config, even when a key with the same name exists.
    assert!(root.resolved_token.is_none());

    let meta = SkipRoot::properties_metadata(None);
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].key, "name");
}

// =========================================================================
// Map-valued sections
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct UpstreamEntry {
    url: String,
    #[config(default = true)]
    enabled: bool,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct MapRoot {
    #[config(section)]
    upstreams: std::collections::HashMap<String, UpstreamEntry>,
    #[config(section)]
    mirrors: std::collections::BTreeMap<String, UpstreamEntry>,
    #[config(section)]
    optional_pools: Option<std::collections::HashMap<String, UpstreamEntry>>,
}

#[test]
fn test_map_section_parses_entries() {
    let yaml = r#"
upstreams:
  npm:
    url: "https://registry.npmjs.org"
  docker:
    url: "https://registry-1.docker.io"
    enabled: false
mirrors:
  eu:
    url: "https://eu.mirror"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let root = MapRoot::from_config(&config, None).unwrap();

    assert_eq!(root.upstreams.len(), 2);
    assert_eq!(root.upstreams["npm"].url, "https://registry.npmjs.org");
    assert!(root.upstreams["npm"].enabled); // default
    assert!(!root.upstreams["docker"].enabled);

    assert_eq!(root.mirrors.len(), 1);
    assert_eq!(root.mirrors["eu"].url, "https://eu.mirror");

    assert!(root.optional_pools.is_none());
}

#[test]
fn test_map_section_absent_is_empty() {
    let config = R2eConfig::empty();
    let root = MapRoot::from_config(&config, None).unwrap();
    assert!(root.upstreams.is_empty());
    assert!(root.mirrors.is_empty());
    assert!(root.optional_pools.is_none());
}

#[test]
fn test_map_section_entry_error_propagates() {
    let yaml = r#"
upstreams:
  npm:
    enabled: false
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let err = MapRoot::from_config(&config, None).unwrap_err();
    assert!(matches!(err, ConfigError::NotFound(key) if key == "upstreams.npm.url"));
}

#[test]
fn test_map_section_with_prefix() {
    let yaml = r#"
app:
  upstreams:
    cargo:
      url: "https://crates.io"
  mirrors: {}
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let root = MapRoot::from_config(&config, Some("app")).unwrap();
    assert_eq!(root.upstreams.len(), 1);
    assert_eq!(root.upstreams["cargo"].url, "https://crates.io");
}

// =========================================================================
// Tagged enum sections
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct S3Settings {
    bucket: String,
    region: Option<String>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct FilesystemSettings {
    #[config(default = "./data/blobs")]
    base_dir: String,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(tag = "backend")]
enum StorageConfig {
    S3(S3Settings),
    #[config(default)]
    Filesystem(FilesystemSettings),
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct StorageRoot {
    #[config(section)]
    storage: StorageConfig,
}

#[test]
fn test_tagged_enum_selects_variant_at_same_prefix() {
    let yaml = r#"
storage:
  backend: s3
  bucket: my-bucket
  region: eu-west-1
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let root = StorageRoot::from_config(&config, None).unwrap();
    match root.storage {
        StorageConfig::S3(s3) => {
            assert_eq!(s3.bucket, "my-bucket");
            assert_eq!(s3.region.as_deref(), Some("eu-west-1"));
        }
        other => panic!("expected S3, got {other:?}"),
    }
}

#[test]
fn test_tagged_enum_default_variant_when_tag_absent() {
    let config = R2eConfig::empty();
    let storage = StorageConfig::from_config(&config, Some("storage")).unwrap();
    match storage {
        StorageConfig::Filesystem(fs) => assert_eq!(fs.base_dir, "./data/blobs"),
        other => panic!("expected Filesystem, got {other:?}"),
    }
}

#[test]
fn test_tagged_enum_unknown_tag_errors() {
    let config = R2eConfig::from_yaml_str("storage:\n  backend: ftp\n").unwrap();
    let err = StorageConfig::from_config(&config, Some("storage")).unwrap_err();
    match err {
        ConfigError::Deserialize { key, message } => {
            assert_eq!(key, "storage.backend");
            assert!(message.contains("ftp"));
            assert!(message.contains("s3, filesystem"));
        }
        other => panic!("expected Deserialize error, got {other:?}"),
    }
}

#[test]
fn test_tagged_enum_variant_payload_error_propagates() {
    // backend=s3 selected but required `bucket` missing
    let config = R2eConfig::from_yaml_str("storage:\n  backend: s3\n").unwrap();
    let err = StorageConfig::from_config(&config, Some("storage")).unwrap_err();
    assert!(matches!(err, ConfigError::NotFound(key) if key == "storage.bucket"));
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
#[config(tag = "mode", rename_all = "kebab-case")]
enum AuthMode {
    ServiceAccount,
    #[config(rename = "passthrough")]
    PassThrough,
    None,
}

#[test]
fn test_tagged_enum_unit_variants_rename_all_and_rename() {
    let config = R2eConfig::from_yaml_str("auth:\n  mode: service-account\n").unwrap();
    assert_eq!(
        AuthMode::from_config(&config, Some("auth")).unwrap(),
        AuthMode::ServiceAccount
    );

    let config = R2eConfig::from_yaml_str("auth:\n  mode: passthrough\n").unwrap();
    assert_eq!(
        AuthMode::from_config(&config, Some("auth")).unwrap(),
        AuthMode::PassThrough
    );
}

#[test]
fn test_tagged_enum_without_default_requires_tag() {
    let config = R2eConfig::empty();
    let err = AuthMode::from_config(&config, Some("auth")).unwrap_err();
    assert!(matches!(err, ConfigError::NotFound(key) if key == "auth.mode"));
}

#[test]
fn test_tagged_enum_metadata_is_tag_key() {
    let meta = StorageConfig::properties_metadata(Some("storage"));
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].key, "backend");
    assert_eq!(meta[0].full_key, "storage.backend");
    assert!(!meta[0].required); // has a default variant
    assert_eq!(meta[0].default_value.as_deref(), Some("filesystem"));

    let meta = AuthMode::properties_metadata(None);
    assert_eq!(meta[0].full_key, "mode");
    assert!(meta[0].required); // no default variant
}

// =========================================================================
// Manual impl surface: NoChildren + default properties_metadata
// =========================================================================

#[derive(Clone, Debug)]
struct ManualDynamicConfig {
    entries: Vec<(String, String)>,
}

impl ConfigProperties for ManualDynamicConfig {
    type Children = r2e_core::config::NoChildren;

    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError> {
        let base = prefix.unwrap_or("entries").to_string();
        let entries = config
            .sub_keys(&base)
            .into_iter()
            .map(|k| {
                let v: String = config.get(&format!("{base}.{k}"))?;
                Ok((k, v))
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        Ok(Self { entries })
    }
}

#[test]
fn test_manual_impl_with_public_surface_only() {
    let yaml = r#"
labels:
  team: core
  env: prod
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let manual = ManualDynamicConfig::from_config(&config, Some("labels")).unwrap();
    let mut entries = manual.entries.clone();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            ("env".to_string(), "prod".to_string()),
            ("team".to_string(), "core".to_string()),
        ]
    );
    // Default properties_metadata is a no-op
    assert!(ManualDynamicConfig::properties_metadata(None).is_empty());
}

#[test]
fn test_optional_section_empty_yaml_key_is_none() {
    // `database:` with no content flattens to a Null at the exact key —
    // treated as absent, not as a present-but-invalid section.
    let config = R2eConfig::from_yaml_str("database:\nname: app\n").unwrap();
    let root = PresenceRoot::from_config(&config, None).unwrap();
    assert!(root.database.is_none());
}

// =========================================================================
// Option<T> + default (review finding: double .into() broke Option<String>)
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct OptionDefaultConfig {
    #[config(default = "en")]
    lang: Option<String>,
    #[config(default = 5)]
    retries: Option<i64>,
}

#[test]
fn test_option_field_with_default() {
    let config = R2eConfig::empty();
    let c = OptionDefaultConfig::from_config(&config, None).unwrap();
    assert_eq!(c.lang.as_deref(), Some("en"));
    assert_eq!(c.retries, Some(5));

    let config = R2eConfig::from_yaml_str("lang: fr\nretries: 1\n").unwrap();
    let c = OptionDefaultConfig::from_config(&config, None).unwrap();
    assert_eq!(c.lang.as_deref(), Some("fr"));
    assert_eq!(c.retries, Some(1));
}

#[test]
fn test_string_default_metadata_is_unquoted() {
    let meta = OptionDefaultConfig::properties_metadata(None);
    let lang = meta.iter().find(|m| m.key == "lang").unwrap();
    assert_eq!(lang.default_value.as_deref(), Some("en")); // not "\"en\""
}

// =========================================================================
// Custom base config file: load_from / load_profiled_from (task #446)
// =========================================================================

fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn load_from_reads_custom_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "patina.yaml", "app:\n  name: patina\n  port: 9000\n");

    let config = R2eConfig::load_from(&file).unwrap();
    assert_eq!(config.get::<String>("app.name").unwrap(), "patina");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 9000);
}

#[test]
fn load_from_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = R2eConfig::load_from(dir.path().join("nope.yaml")).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg) if msg.contains("nope.yaml")),
        "expected Load error naming the file, got: {err}"
    );
}

#[test]
fn load_profiled_from_overlays_derived_profile_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "patina.yaml", "app:\n  name: patina\n  port: 9000\n");
    // The overlay file name derives from the base name, not `application-*`.
    write_file(dir.path(), "patina-test.yaml", "app:\n  port: 1234\n");

    let config = R2eConfig::load_profiled_from(&file, Some("test")).unwrap();
    assert_eq!(config.get::<String>("app.name").unwrap(), "patina");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 1234);
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}

#[test]
fn load_profiled_from_reads_profile_from_base_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "r2e:\n  profile: staging\napp:\n  port: 9000\n",
    );
    write_file(dir.path(), "patina-staging.yaml", "app:\n  port: 4321\n");

    // Skip when R2E_PROFILE is set in the environment: it wins over the
    // r2e.profile key and would overlay a different (absent) sibling.
    if std::env::var("R2E_PROFILE").is_err() {
        let config = R2eConfig::load_profiled_from(&file, None).unwrap();
        assert_eq!(config.get::<i64>("app.port").unwrap(), 4321);
    }
}

#[test]
fn load_profiled_from_tolerates_missing_profile_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "patina.yaml", "app:\n  port: 9000\n");

    let config = R2eConfig::load_profiled_from(&file, Some("test")).unwrap();
    assert_eq!(config.get::<i64>("app.port").unwrap(), 9000);
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}

#[test]
fn load_from_resolves_secret_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "app:\n  secret: \"${R2E_TEST_UNSET_446:fallback}\"\n",
    );

    let config = R2eConfig::load_from(&file).unwrap();
    assert_eq!(config.get::<String>("app.secret").unwrap(), "fallback");
}

#[test]
fn load_from_directory_path_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let err = R2eConfig::load_from(dir.path()).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg) if msg.contains("not a regular file")),
        "expected clear not-a-file error, got: {err}"
    );
}
