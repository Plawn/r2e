// Tests for `validate_section` / `validate_keys` (src/config/validation.rs).

use r2e_core::config::{validate_keys, validate_section, R2eConfig};

// Regression: a kebab-case key like `database.min-idle` is NOT addressable
// via an `R2E_` env var (the strict overlay maps only `_`→`.`), so the hint
// must be `None` and the message must point at YAML/placeholders — not the
// misleading `R2E_DATABASE_MIN_IDLE`.
#[test]
fn test_validate_keys_dashed_key_has_no_env_hint() {
    let config = R2eConfig::empty();
    let errors = validate_keys(&config, &[("src", "database.min-idle", "u32")]);
    assert_eq!(errors.len(), 1, "absent key must be reported: {errors:?}");
    assert_eq!(errors[0].env_hint, None, "dashed key is not env-reachable");
    let rendered = errors[0].to_string();
    assert!(
        rendered.contains("application.yaml") && !rendered.contains("set env var"),
        "dashed-key message must be YAML/placeholder-only: {rendered}"
    );
}

// Same for a snake_case key: `R2E_DATABASE_MAX_IDLE` would insert
// `database.max.idle`, never `database.max_idle` — suggesting it would be a
// lie, so the hint must be `None` too.
#[test]
fn test_validate_keys_snake_key_has_no_env_hint() {
    let config = R2eConfig::empty();
    let errors = validate_keys(&config, &[("src", "database.max_idle", "u32")]);
    assert_eq!(errors.len(), 1, "absent key must be reported: {errors:?}");
    assert_eq!(errors[0].env_hint, None, "snake key is not env-reachable");
}

// A purely dotted (env-reachable) key names its full working var —
// `R2E_` prefix included, since unprefixed env vars are ignored.
#[test]
fn test_validate_keys_dotted_key_has_prefixed_env_hint() {
    let config = R2eConfig::empty();
    let errors = validate_keys(&config, &[("src", "database.pool.size", "u32")]);
    assert_eq!(errors.len(), 1, "absent key must be reported: {errors:?}");
    assert_eq!(errors[0].env_hint.as_deref(), Some("R2E_DATABASE_POOL_SIZE"));
    let rendered = errors[0].to_string();
    assert!(
        rendered.contains("set env var `R2E_DATABASE_POOL_SIZE`"),
        "dotted-key message must name the full R2E_-prefixed var: {rendered}"
    );
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct EnvOnlyConfig {
    #[config(env = "TEST_R2E_VALIDATION_ENV_ONLY")]
    pub url: String,
}

#[test]
fn test_validate_section_env_var_satisfies_required_field() {
    std::env::set_var("TEST_R2E_VALIDATION_ENV_ONLY", "postgres://from-env/db");

    let config = R2eConfig::empty();
    let errors = validate_section::<EnvOnlyConfig>(&config, Some("db"));

    std::env::remove_var("TEST_R2E_VALIDATION_ENV_ONLY");
    assert!(
        errors.is_empty(),
        "env-only required field must not be reported missing: {errors:?}"
    );
}

// Never set by any test — exercises the "env var absent" paths.
#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct EnvUnsetConfig {
    #[config(env = "TEST_R2E_VALIDATION_ENV_UNSET")]
    pub url: String,
}

#[test]
fn test_validate_section_still_fails_without_key_or_env() {
    let config = R2eConfig::empty();
    let errors = validate_section::<EnvUnsetConfig>(&config, Some("db"));
    assert_eq!(errors.len(), 1, "expected one missing key: {errors:?}");
    assert_eq!(errors[0].key, "db.url");
    assert_eq!(
        errors[0].env_hint.as_deref(),
        Some("TEST_R2E_VALIDATION_ENV_UNSET")
    );
}

#[test]
fn test_validate_section_key_in_map_passes_without_env() {
    let yaml = r#"
db:
  url: "postgres://from-yaml/db"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let errors = validate_section::<EnvUnsetConfig>(&config, Some("db"));
    assert!(
        errors.is_empty(),
        "key present in map must pass: {errors:?}"
    );
}

// The generated `PropertyMeta::resolvable` probe is the single resolution
// oracle consumed by `validate_section` — exercise it directly.
#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ProbeConfig {
    #[config(env = "TEST_R2E_VALIDATION_PROBE")]
    pub url: String,
    pub name: String,
}

#[test]
fn test_property_meta_resolvable_probe() {
    use r2e_core::config::ConfigProperties;

    let empty = R2eConfig::empty();
    let meta = ProbeConfig::properties_metadata(Some("db"));
    let url = meta.iter().find(|m| m.key == "url").unwrap();
    let name = meta.iter().find(|m| m.key == "name").unwrap();

    // Nothing set: neither property resolves.
    assert!(!url.is_resolvable(&empty));
    assert!(!name.is_resolvable(&empty));

    // Custom env var satisfies only the `#[config(env)]` property.
    std::env::set_var("TEST_R2E_VALIDATION_PROBE", "postgres://from-env/db");
    let url_via_env = url.is_resolvable(&empty);
    std::env::remove_var("TEST_R2E_VALIDATION_PROBE");
    assert!(url_via_env);

    // Key in the config map satisfies a plain property.
    let config = R2eConfig::from_yaml_str("db:\n  name: \"orders\"").unwrap();
    assert!(name.is_resolvable(&config));
    assert!(!url.is_resolvable(&config));
}

// Section probe: a `#[config(section)]` property resolves when any key lives
// under its prefix (`has_prefix`), mirroring from_config's presence check.
// Unreachable through validate_section (sections are never required) but part
// of the public `is_resolvable` oracle, so pin its semantics here.
#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ProbeChildConfig {
    pub host: String,
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ProbeParentConfig {
    #[config(section)]
    pub child: ProbeChildConfig,
}

#[test]
fn test_property_meta_resolvable_probe_section() {
    use r2e_core::config::ConfigProperties;

    let meta = ProbeParentConfig::properties_metadata(Some("app"));
    let child = meta.iter().find(|m| m.key == "child").unwrap();
    assert!(child.is_section);

    assert!(!child.is_resolvable(&R2eConfig::empty()));

    let config = R2eConfig::from_yaml_str("app:\n  child:\n    host: \"localhost\"").unwrap();
    assert!(child.is_resolvable(&config));
}

// ── Nested-section phase-1 reporting (task #660) ────────────────────────────
// `validate_section` recurses into `#[config(section)]` props via the
// derive-generated `PropertyMeta::validate_nested` hook: ALL nested missing
// required keys are reported in one pass with full metadata, instead of the
// phase-2 from_config probe short-circuiting on the first `NotFound`.

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct NestedLeafConfig {
    /// Connection URL for the nested service.
    pub url: String,
    pub port: i64,
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct NestedParentConfig {
    pub name: String,
    #[config(section)]
    pub db: NestedLeafConfig,
}

#[test]
fn test_validate_section_reports_all_nested_missing_keys_with_metadata() {
    let config = R2eConfig::empty();
    let errors = validate_section::<NestedParentConfig>(&config, Some("app"));

    let keys: Vec<&str> = errors.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["app.name", "app.db.url", "app.db.port"],
        "one pass must report the parent key and every nested key: {errors:?}"
    );

    let url = errors.iter().find(|e| e.key == "app.db.url").unwrap();
    assert_eq!(url.source, "app.db");
    assert_eq!(url.expected_type, "String");
    assert_eq!(url.env_hint.as_deref(), Some("R2E_APP_DB_URL"));
    assert_eq!(
        url.description.as_deref(),
        Some("Connection URL for the nested service.")
    );

    let port = errors.iter().find(|e| e.key == "app.db.port").unwrap();
    assert_eq!(port.expected_type, "i64");
}

#[test]
fn test_validate_section_nested_passes_when_all_keys_present() {
    let yaml = r#"
app:
  name: "svc"
  db:
    url: "postgres://localhost/db"
    port: 5432
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let errors = validate_section::<NestedParentConfig>(&config, Some("app"));
    assert!(
        errors.is_empty(),
        "fully-populated nested config: {errors:?}"
    );
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DeepParentConfig {
    #[config(section)]
    pub middle: NestedParentConfig,
}

#[test]
fn test_validate_section_recurses_through_multiple_levels() {
    let config = R2eConfig::empty();
    let errors = validate_section::<DeepParentConfig>(&config, Some("root"));

    let keys: Vec<&str> = errors.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "root.middle.name",
            "root.middle.db.url",
            "root.middle.db.port"
        ],
        "grandchild keys must be reported too: {errors:?}"
    );
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct OptionalSectionConfig {
    #[config(section)]
    pub db: Option<NestedLeafConfig>,
}

#[test]
fn test_validate_section_skips_absent_optional_section() {
    let config = R2eConfig::empty();
    let errors = validate_section::<OptionalSectionConfig>(&config, Some("app"));
    assert!(
        errors.is_empty(),
        "an absent optional section is legal (from_config yields None): {errors:?}"
    );
}

#[test]
fn test_validate_section_validates_present_optional_section() {
    let config = R2eConfig::from_yaml_str("app:\n  db:\n    port: 5432").unwrap();
    let errors = validate_section::<OptionalSectionConfig>(&config, Some("app"));
    assert_eq!(
        errors.len(),
        1,
        "present optional section must be validated: {errors:?}"
    );
    assert_eq!(errors[0].key, "app.db.url");
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default)]
struct DefaultableLeafConfig {
    pub host: String,
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DefaultedSectionConfig {
    #[config(section, default)]
    pub cache: DefaultableLeafConfig,
}

#[test]
fn test_validate_section_skips_absent_defaulted_section() {
    let config = R2eConfig::empty();
    let errors = validate_section::<DefaultedSectionConfig>(&config, Some("app"));
    assert!(
        errors.is_empty(),
        "an absent #[config(section, default)] falls back to Default: {errors:?}"
    );
}

#[test]
fn test_validate_section_validates_present_defaulted_section() {
    let config = R2eConfig::from_yaml_str("app:\n  cache:\n    ttl: 30").unwrap();
    let errors = validate_section::<DefaultedSectionConfig>(&config, Some("app"));
    assert_eq!(
        errors.len(),
        1,
        "present defaulted section must be validated: {errors:?}"
    );
    assert_eq!(errors[0].key, "app.cache.host");
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct MapSectionConfig {
    #[config(section)]
    pub endpoints: std::collections::HashMap<String, NestedLeafConfig>,
}

#[test]
fn test_validate_section_validates_each_map_section_entry() {
    let yaml = r#"
app:
  endpoints:
    orders:
      url: "http://orders"
      port: 80
    billing:
      port: 443
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let errors = validate_section::<MapSectionConfig>(&config, Some("app"));
    assert_eq!(
        errors.len(),
        1,
        "only the incomplete entry must report: {errors:?}"
    );
    assert_eq!(errors[0].key, "app.endpoints.billing.url");
    assert_eq!(errors[0].source, "app.endpoints.billing");
}

#[test]
fn test_validate_section_skips_absent_map_section() {
    let config = R2eConfig::empty();
    let errors = validate_section::<MapSectionConfig>(&config, Some("app"));
    assert!(
        errors.is_empty(),
        "an absent map section is an empty map, nothing to validate: {errors:?}"
    );
}

// A serde-backed FromConfigValue type: conversion failures surface as
// ConfigError::Deserialize, which validate_section must report, not swallow.
#[derive(serde::Deserialize, r2e_macros::FromConfigValue, Clone, Debug)]
#[serde(rename_all = "lowercase")]
enum LogMode {
    #[allow(dead_code)]
    Plain,
    #[allow(dead_code)]
    Json,
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct EnvDeserializeConfig {
    #[config(env = "TEST_R2E_VALIDATION_ENV_BOGUS")]
    pub mode: LogMode,
}

#[test]
fn test_validate_section_reports_bad_env_value_as_deserialize_error() {
    std::env::set_var("TEST_R2E_VALIDATION_ENV_BOGUS", "not-a-mode");

    let config = R2eConfig::empty();
    let errors = validate_section::<EnvDeserializeConfig>(&config, Some("log"));

    std::env::remove_var("TEST_R2E_VALIDATION_ENV_BOGUS");
    assert_eq!(
        errors.len(),
        1,
        "bad env value must surface a validation error: {errors:?}"
    );
    assert_eq!(errors[0].key, "log.mode");
}
