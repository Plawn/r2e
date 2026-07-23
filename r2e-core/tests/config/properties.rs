//! `#[derive(ConfigProperties)]`: key mapping, defaults, `skip`, metadata, manual impls.

use r2e_core::config::{ConfigError, ConfigProperties, R2eConfig};

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

    let client_meta = meta
        .iter()
        .find(|m| m.full_key == "oidc.client.id")
        .unwrap();
    assert_eq!(client_meta.key, "client.id");
    assert!(!client_meta.required); // has default
    assert!(client_meta.default_value.is_some());
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
