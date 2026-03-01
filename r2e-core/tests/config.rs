use r2e_core::config::{ConfigProperties, ConfigValue, R2eConfig};

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
