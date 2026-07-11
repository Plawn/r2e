// Tests for `validate_section` / `validate_keys` (src/config/validation.rs).

use r2e_core::config::{validate_section, R2eConfig};

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
    assert_eq!(errors[0].env_hint, "TEST_R2E_VALIDATION_ENV_UNSET");
}

#[test]
fn test_validate_section_key_in_map_passes_without_env() {
    let yaml = r#"
db:
  url: "postgres://from-yaml/db"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let errors = validate_section::<EnvUnsetConfig>(&config, Some("db"));
    assert!(errors.is_empty(), "key present in map must pass: {errors:?}");
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
