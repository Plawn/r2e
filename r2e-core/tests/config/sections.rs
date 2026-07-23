//! `#[config(section)]`: nesting, optionality, defaults, maps, tagged enums.

use r2e_core::config::{ConfigError, ConfigProperties, R2eConfig};

// =========================================================================
// ConfigProperties — #[config(section)] nesting
// =========================================================================

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct NestedDbConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct AppConfig {
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
pub struct ServerConfig {
    pub host: String,
    #[config(section)]
    pub tls: Option<TlsConfig>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct TlsConfig {
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

// =======================================================================
// PropertyMeta / prefix / presence, against the fixtures above
// =======================================================================

#[test]
fn test_property_meta_section_flag() {
    let meta = AppConfig::properties_metadata(Some("app"));
    let db_meta = meta.iter().find(|m| m.key == "database").unwrap();
    assert!(db_meta.is_section);
    let name_meta = meta.iter().find(|m| m.key == "name").unwrap();
    assert!(!name_meta.is_section);
}

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

#[test]
fn test_optional_section_empty_yaml_key_is_none() {
    // `database:` with no content flattens to a Null at the exact key —
    // treated as absent, not as a present-but-invalid section.
    let config = R2eConfig::from_yaml_str("database:\nname: app\n").unwrap();
    let root = PresenceRoot::from_config(&config, None).unwrap();
    assert!(root.database.is_none());
}
