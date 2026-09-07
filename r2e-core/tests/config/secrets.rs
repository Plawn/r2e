use r2e_core::config::secrets::{resolve_placeholders, DefaultSecretResolver};

#[test]
fn test_env_var_resolution() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_DB_URL", "postgres://localhost/test") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_R2E_DB_URL}", &resolver).unwrap();
    assert_eq!(result, "postgres://localhost/test");
    unsafe { std::env::remove_var("TEST_R2E_DB_URL") };
}

#[test]
fn test_explicit_env_resolution() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_HOST", "myhost") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${env:TEST_R2E_HOST}", &resolver).unwrap();
    assert_eq!(result, "myhost");
    unsafe { std::env::remove_var("TEST_R2E_HOST") };
}

#[test]
fn test_mixed_resolution() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_MIX_HOST", "localhost") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("http://${TEST_R2E_MIX_HOST}:8080/api", &resolver).unwrap();
    assert_eq!(result, "http://localhost:8080/api");
    unsafe { std::env::remove_var("TEST_R2E_MIX_HOST") };
}

#[test]
fn test_no_placeholder() {
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("plain-value", &resolver).unwrap();
    assert_eq!(result, "plain-value");
}

#[test]
fn test_unclosed_placeholder() {
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${UNCLOSED", &resolver);
    assert!(result.is_err());
}

#[test]
fn test_file_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = dir.path().join("secret.txt");
    std::fs::write(&secret_file, "my-secret-value\n").unwrap();

    let resolver = DefaultSecretResolver;
    let ref_str = format!("${{file:{}}}", secret_file.display());
    let result = resolve_placeholders(&ref_str, &resolver).unwrap();
    assert_eq!(result, "my-secret-value");
}

#[test]
fn test_default_value_when_var_missing() {
    let _env = crate::support::env_lock();
    // Ensure the var does not exist
    unsafe { std::env::remove_var("TEST_R2E_MISSING_VAR") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_R2E_MISSING_VAR:fallback}", &resolver).unwrap();
    assert_eq!(result, "fallback");
}

#[test]
fn test_default_value_not_used_when_var_exists() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_EXISTS_VAR", "real-value") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_R2E_EXISTS_VAR:fallback}", &resolver).unwrap();
    assert_eq!(result, "real-value");
    unsafe { std::env::remove_var("TEST_R2E_EXISTS_VAR") };
}

#[test]
fn test_default_value_with_env_prefix() {
    let _env = crate::support::env_lock();
    unsafe { std::env::remove_var("TEST_R2E_ENV_DEF") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${env:TEST_R2E_ENV_DEF:env-default}", &resolver).unwrap();
    assert_eq!(result, "env-default");
}

#[test]
fn test_default_value_empty_string() {
    let _env = crate::support::env_lock();
    unsafe { std::env::remove_var("TEST_R2E_EMPTY_DEF") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_R2E_EMPTY_DEF:}", &resolver).unwrap();
    assert_eq!(result, "");
}

// ── Placeholders inside containers ─────────────────────────────────────
//
// A YAML sequence is flattened into one `ConfigValue::List` under the parent
// key, so a placeholder inside a list is not a top-level string value. It
// used to survive resolution verbatim, which silently handed plugins the
// literal `"${VAR}"` (`mcp.allowed-hosts` being the case that surfaced it).

#[test]
fn resolves_placeholders_inside_list_values() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_LIST_HOST", "api.example.com") };

    let mut config = r2e_core::R2eConfig::from_yaml_str(
        r#"
mcp:
  allowed-hosts:
    - ${TEST_R2E_LIST_HOST}
    - ${TEST_R2E_LIST_MISSING:fallback.example.com}
    - literal.example.com
"#,
    )
    .unwrap();
    config
        .resolve_placeholders_with(&DefaultSecretResolver)
        .unwrap();

    assert_eq!(
        config.get::<Vec<String>>("mcp.allowed-hosts").unwrap(),
        vec![
            "api.example.com".to_string(),
            "fallback.example.com".to_string(),
            "literal.example.com".to_string(),
        ]
    );

    unsafe { std::env::remove_var("TEST_R2E_LIST_HOST") };
}

#[test]
fn resolves_placeholders_inside_nested_map_values() {
    let _env = crate::support::env_lock();
    unsafe { std::env::set_var("TEST_R2E_NESTED_AUD", "https://api.example.com/mcp") };

    let mut config = r2e_core::R2eConfig::from_yaml_str(
        r#"
mcp:
  auth:
    extra-authorize-params:
      audience: ${TEST_R2E_NESTED_AUD}
"#,
    )
    .unwrap();
    config
        .resolve_placeholders_with(&DefaultSecretResolver)
        .unwrap();

    assert_eq!(
        config
            .get::<String>("mcp.auth.extra-authorize-params.audience")
            .unwrap(),
        "https://api.example.com/mcp"
    );

    unsafe { std::env::remove_var("TEST_R2E_NESTED_AUD") };
}
