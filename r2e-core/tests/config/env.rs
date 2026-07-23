//! Environment overrides: `#[config(env = "…")]` and the `R2E_` overlay.

use r2e_core::config::{ConfigProperties, ConfigValue, R2eConfig};

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
    let _env = crate::support::env_lock();
    std::env::set_var("TEST_R2E_DATABASE_URL", "postgres://from-env/mydb");

    let config = R2eConfig::empty();
    let db = DbEnvConfig::from_config(&config, Some("db")).unwrap();
    assert_eq!(db.url, "postgres://from-env/mydb");

    std::env::remove_var("TEST_R2E_DATABASE_URL");
}

#[test]
fn test_config_env_override_yaml_takes_priority() {
    let _env = crate::support::env_lock();
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

// The overlay mapping is deliberately STRICT — no fuzzy/relaxed matching
// against existing keys (a silently mis-mapped setting is worse than an
// unaddressable one). `R2E_SECURITY_JWT_JWKS_URL` derives
// `security.jwt.jwks.url` and must NOT touch a kebab-case
// `security.jwt.jwks-url` (or a snake_case `jwks_url`): such keys are only
// settable via YAML / `${VAR}` placeholders, and the validation hints say so.
#[test]
fn test_env_overlay_is_strict_never_touches_kebab_or_snake_keys() {
    use std::collections::HashMap;
    let mut values: HashMap<String, ConfigValue> = HashMap::new();
    values.insert(
        "security.jwt.jwks-url".to_string(),
        ConfigValue::String("from-yaml".to_string()),
    );
    values.insert(
        "app.jwks_url".to_string(),
        ConfigValue::String("from-yaml".to_string()),
    );

    let env = vec![
        (
            "R2E_SECURITY_JWT_JWKS_URL".to_string(),
            "from-env".to_string(),
        ),
        ("R2E_APP_JWKS_URL".to_string(), "from-env".to_string()),
    ];
    R2eConfig::apply_env_overlay_for_test(&mut values, env);

    let mut cfg = R2eConfig::empty();
    for (k, v) in values {
        cfg.set(&k, v);
    }
    // Existing keys untouched; the env vars land on their strict dotted keys.
    assert_eq!(
        cfg.get::<String>("security.jwt.jwks-url").unwrap(),
        "from-yaml"
    );
    assert_eq!(cfg.get::<String>("app.jwks_url").unwrap(), "from-yaml");
    assert_eq!(
        cfg.get::<String>("security.jwt.jwks.url").unwrap(),
        "from-env"
    );
    assert_eq!(cfg.get::<String>("app.jwks.url").unwrap(), "from-env");
}

// =======================================================================
// PropertyMeta — the env-var field
// =======================================================================

#[test]
fn test_property_meta_env_var() {
    let meta = DbEnvConfig::properties_metadata(Some("db"));
    let url_meta = meta.iter().find(|m| m.key == "url").unwrap();
    assert_eq!(url_meta.env_var.as_deref(), Some("TEST_R2E_DATABASE_URL"));
}
