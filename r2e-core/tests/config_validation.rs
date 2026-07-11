// Tests for `validate_section` / `validate_keys` (src/config/validation.rs).

use r2e_core::config::{validate_section, ConfigProperties, R2eConfig};

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
    assert!(
        errors.is_empty(),
        "env-only required field must not be reported missing: {errors:?}"
    );

    std::env::remove_var("TEST_R2E_VALIDATION_ENV_ONLY");
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct EnvMissingConfig {
    #[config(env = "TEST_R2E_VALIDATION_ENV_UNSET")]
    pub url: String,
}

#[test]
fn test_validate_section_still_fails_without_key_or_env() {
    std::env::remove_var("TEST_R2E_VALIDATION_ENV_UNSET");

    let config = R2eConfig::empty();
    let errors = validate_section::<EnvMissingConfig>(&config, Some("db"));
    assert_eq!(errors.len(), 1, "expected one missing key: {errors:?}");
    assert_eq!(errors[0].key, "db.url");
    assert_eq!(errors[0].env_hint, "TEST_R2E_VALIDATION_ENV_UNSET");
}

#[allow(dead_code)]
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct EnvOverrideConfig {
    #[config(env = "TEST_R2E_VALIDATION_ENV_YAML")]
    pub url: String,
}

#[test]
fn test_validate_section_key_in_map_passes_without_env() {
    std::env::remove_var("TEST_R2E_VALIDATION_ENV_YAML");

    let yaml = r#"
db:
  url: "postgres://from-yaml/db"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let errors = validate_section::<EnvOverrideConfig>(&config, Some("db"));
    assert!(errors.is_empty(), "key present in map must pass: {errors:?}");
}
