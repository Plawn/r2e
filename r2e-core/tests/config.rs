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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();

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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
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
#[config(prefix = "app.database")]
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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let db = DatabaseConfig::from_config(&config).unwrap();

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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let db = DatabaseConfig::from_config(&config).unwrap();

    assert_eq!(db.url, "postgres://localhost/mydb");
    assert_eq!(db.pool_size, 50); // yaml overrides default
    assert_eq!(db.timeout, Some(30));
}

#[test]
fn test_config_properties_basic_missing_required() {
    let config = R2eConfig::empty();
    let result = DatabaseConfig::from_config(&config);

    assert!(result.is_err()); // "url" is required
}

// --- ConfigProperties: #[config(key = "...")] custom key mapping ---
//
// When env vars like OIDC_JWKS_URL produce config key "oidc.jwks.url",
// but the Rust field is named `jwks_url` (which would generate "oidc.jwks_url"),
// use #[config(key = "jwks.url")] to override the generated key.

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "oidc")]
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
    // YAML nesting matches the dotted key, not the Rust field name:
    //   oidc.jwks.url  → oidc: { jwks: { url: ... } }
    //   oidc.client.id → oidc: { client: { id: ... } }
    let yaml = r#"
oidc:
  issuer: "https://auth.example.com"
  jwks:
    url: "https://auth.example.com/.well-known/jwks.json"
  client:
    id: "custom-client"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let oidc = OidcTestConfig::from_config(&config).unwrap();

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
    // All fields optional or defaulted → works with empty config
    let config = R2eConfig::empty();
    let oidc = OidcTestConfig::from_config(&config).unwrap();

    assert!(oidc.issuer.is_none());
    assert!(oidc.jwks_url.is_none());
    assert_eq!(oidc.audience, "my-app");
    assert_eq!(oidc.client_id, "my-app");
}

#[test]
fn test_config_properties_custom_key_metadata() {
    // Metadata reflects custom keys, not Rust field names
    let meta = OidcTestConfig::properties_metadata();

    let jwks_meta = meta.iter().find(|m| m.full_key == "oidc.jwks.url").unwrap();
    assert_eq!(jwks_meta.key, "jwks.url");
    assert!(!jwks_meta.required);

    let client_meta = meta.iter().find(|m| m.full_key == "oidc.client.id").unwrap();
    assert_eq!(client_meta.key, "client.id");
    assert!(!client_meta.required); // has default
    assert!(client_meta.default_value.is_some());
}

// =========================================================================
// R2eConfig<T> — typed config with Deref access
// =========================================================================

// Re-use DatabaseConfig from above for typed config tests.

#[test]
fn test_with_typed_basic() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test")
        .unwrap()
        .with_typed::<DatabaseConfig>()
        .unwrap();

    // Typed access via Deref
    assert_eq!(config.url, "postgres://localhost/mydb");
    assert_eq!(config.pool_size, 10); // default
    assert!(config.timeout.is_none());

    // Raw access still works
    assert_eq!(
        config.get::<String>("app.database.url").unwrap(),
        "postgres://localhost/mydb"
    );
}

#[test]
fn test_with_typed_profile() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "staging")
        .unwrap()
        .with_typed::<DatabaseConfig>()
        .unwrap();

    assert_eq!(config.profile(), "staging");
}

#[test]
fn test_with_typed_missing_required() {
    let config = R2eConfig::empty();
    let result = config.with_typed::<DatabaseConfig>();
    assert!(result.is_err());
}

#[test]
fn test_raw_downgrade() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
"#;
    let typed_config = R2eConfig::from_yaml_str(yaml, "test")
        .unwrap()
        .with_typed::<DatabaseConfig>()
        .unwrap();

    let raw = typed_config.raw();
    assert_eq!(
        raw.get::<String>("app.database.url").unwrap(),
        "postgres://localhost/mydb"
    );
    assert_eq!(raw.profile(), "test");
}

#[test]
fn test_typed_accessor() {
    let yaml = r#"
app:
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test")
        .unwrap()
        .with_typed::<DatabaseConfig>()
        .unwrap();

    let db: &DatabaseConfig = config.typed();
    assert_eq!(db.url, "postgres://localhost/mydb");
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
    // YAML loader flattens maps to dotted keys, so we set a Map value directly.
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
#[config(prefix = "database")]
struct NestedDbConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let app = AppConfig::from_config(&config).unwrap();

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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let app = AppConfig::from_config(&config).unwrap();

    assert_eq!(app.database.pool_size, 5); // default from NestedDbConfig
}

#[test]
fn test_config_section_standalone() {
    // NestedDbConfig can also be used standalone with its own prefix
    let yaml = r#"
database:
  url: "sqlite::memory:"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let db = NestedDbConfig::from_config(&config).unwrap();

    assert_eq!(db.url, "sqlite::memory:");
    assert_eq!(db.pool_size, 5);
}

#[test]
fn test_config_section_with_typed() {
    let yaml = r#"
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test")
        .unwrap()
        .with_typed::<AppConfig>()
        .unwrap();

    // Typed access via Deref
    assert_eq!(config.name, "my-app");
    assert_eq!(config.database.url, "postgres://localhost/mydb");
    assert_eq!(config.database.pool_size, 5);
}

// =========================================================================
// Optional section
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "server")]
struct ServerConfig {
    pub host: String,
    #[config(section)]
    pub tls: Option<TlsConfig>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "tls")]
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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let srv = ServerConfig::from_config(&config).unwrap();

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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let srv = ServerConfig::from_config(&config).unwrap();

    assert_eq!(srv.host, "0.0.0.0");
    assert!(srv.tls.is_none());
}

// =========================================================================
// #[config(env = "...")] — explicit env var override
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "db")]
struct DbEnvConfig {
    #[config(env = "TEST_R2E_DATABASE_URL")]
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[test]
fn test_config_env_override() {
    // Set the env var
    std::env::set_var("TEST_R2E_DATABASE_URL", "postgres://from-env/mydb");

    let config = R2eConfig::empty();
    let db = DbEnvConfig::from_config(&config).unwrap();
    assert_eq!(db.url, "postgres://from-env/mydb");

    std::env::remove_var("TEST_R2E_DATABASE_URL");
}

#[test]
fn test_config_env_override_yaml_takes_priority() {
    // If the key is in the config, env var is not used
    std::env::set_var("TEST_R2E_DATABASE_URL", "postgres://from-env/mydb");

    let yaml = r#"
db:
  url: "postgres://from-yaml/mydb"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let db = DbEnvConfig::from_config(&config).unwrap();
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
    let meta = DbEnvConfig::properties_metadata();
    let url_meta = meta.iter().find(|m| m.key == "url").unwrap();
    assert_eq!(url_meta.env_var.as_deref(), Some("TEST_R2E_DATABASE_URL"));
}

#[test]
fn test_property_meta_section_flag() {
    let meta = AppConfig::properties_metadata();
    let db_meta = meta.iter().find(|m| m.key == "database").unwrap();
    assert!(db_meta.is_section);
    let name_meta = meta.iter().find(|m| m.key == "name").unwrap();
    assert!(!name_meta.is_section);
}

// =========================================================================
// from_config_prefixed — runtime prefix override
// =========================================================================

#[test]
fn test_from_config_prefixed() {
    let yaml = r#"
custom:
  prefix:
    url: "postgres://custom/mydb"
    pool_size: 42
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    // Use a different prefix than the struct's declared "database"
    let db = NestedDbConfig::from_config_prefixed(&config, "custom.prefix").unwrap();

    assert_eq!(db.url, "postgres://custom/mydb");
    assert_eq!(db.pool_size, 42);
}

// =========================================================================
// ConfigProperties with u16 field
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
#[config(prefix = "server")]
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
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let srv = PortConfig::from_config(&config).unwrap();
    assert_eq!(srv.port, 8080);
}

#[test]
fn test_config_properties_u16_default() {
    let config = R2eConfig::empty();
    let srv = PortConfig::from_config(&config).unwrap();
    assert_eq!(srv.port, 3000);
}
