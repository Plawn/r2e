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

// ── Default syntax, nesting, cycles, missing variables (#1472) ─────────────

use r2e_core::config::ConfigError;

fn resolve(value: &str) -> Result<String, ConfigError> {
    resolve_placeholders(value, &DefaultSecretResolver)
}

#[test]
fn default_is_verbatim_after_the_first_colon() {
    let _env = crate::support::env_lock();
    crate::support::remove_env("TEST_R2E_1472_UNSET");

    // No shell-style `:-` operator: the dash belongs to the default.
    assert_eq!(resolve("${TEST_R2E_1472_UNSET:-1}").unwrap(), "-1");
    assert_eq!(resolve("${TEST_R2E_1472_UNSET:-fallback}").unwrap(), "-fallback");
    assert_eq!(resolve("${env:TEST_R2E_1472_UNSET:-1}").unwrap(), "-1");
    assert_eq!(resolve("${TEST_R2E_1472_UNSET:}").unwrap(), "");
    assert_eq!(resolve("${env:TEST_R2E_1472_UNSET:}").unwrap(), "");
    // Later colons are part of the default.
    assert_eq!(
        resolve("${TEST_R2E_1472_UNSET:http://localhost:8080}").unwrap(),
        "http://localhost:8080"
    );
}

#[test]
fn a_variable_set_to_empty_stays_empty() {
    let _env = crate::support::env_lock();
    crate::support::set_env("TEST_R2E_1472_EMPTY", "");
    let plain = resolve("[${TEST_R2E_1472_EMPTY:fallback}]");
    let prefixed = resolve("[${env:TEST_R2E_1472_EMPTY:fallback}]");
    crate::support::remove_env("TEST_R2E_1472_EMPTY");

    assert_eq!(plain.unwrap(), "[]");
    assert_eq!(prefixed.unwrap(), "[]");
}

#[test]
fn a_default_may_hold_a_placeholder() {
    let _env = crate::support::env_lock();
    crate::support::remove_env("TEST_R2E_1472_OUTER");
    crate::support::remove_env("TEST_R2E_1472_UNSET");
    crate::support::set_env("TEST_R2E_1472_INNER", "inner");
    let nested = resolve("a-${TEST_R2E_1472_OUTER:${TEST_R2E_1472_INNER}}-z");
    let twice = resolve("${TEST_R2E_1472_OUTER:${TEST_R2E_1472_UNSET:deep}}");
    crate::support::remove_env("TEST_R2E_1472_INNER");

    assert_eq!(nested.unwrap(), "a-inner-z");
    assert_eq!(twice.unwrap(), "deep");
}

#[test]
fn a_default_may_hold_braces() {
    let _env = crate::support::env_lock();
    crate::support::remove_env("TEST_R2E_1472_UNSET");
    assert_eq!(
        resolve(r#"${TEST_R2E_1472_UNSET:{"a":1}}"#).unwrap(),
        r#"{"a":1}"#
    );
}

#[test]
fn a_resolved_value_is_rendered_in_turn() {
    let _env = crate::support::env_lock();
    crate::support::set_env("TEST_R2E_1472_HOST", "db.internal");
    crate::support::set_env("TEST_R2E_1472_URL", "postgres://${TEST_R2E_1472_HOST}/app");
    let result = resolve("${TEST_R2E_1472_URL}");
    crate::support::remove_env("TEST_R2E_1472_HOST");
    crate::support::remove_env("TEST_R2E_1472_URL");

    assert_eq!(result.unwrap(), "postgres://db.internal/app");
}

#[test]
fn a_self_containing_variable_is_a_named_error() {
    let _env = crate::support::env_lock();
    crate::support::set_env("TEST_R2E_1472_SELF", "x${TEST_R2E_1472_SELF}");
    let result = resolve("${TEST_R2E_1472_SELF}");
    crate::support::remove_env("TEST_R2E_1472_SELF");

    let err = result.unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg)
            if msg.contains("TEST_R2E_1472_SELF -> TEST_R2E_1472_SELF")),
        "expected a cycle error naming the chain, got: {err}"
    );
}

#[test]
fn a_cycle_through_two_variables_names_the_chain() {
    let _env = crate::support::env_lock();
    crate::support::set_env("TEST_R2E_1472_A", "${TEST_R2E_1472_B}");
    crate::support::set_env("TEST_R2E_1472_B", "${env:TEST_R2E_1472_A:unused}");
    let result = resolve("${TEST_R2E_1472_A}");
    crate::support::remove_env("TEST_R2E_1472_A");
    crate::support::remove_env("TEST_R2E_1472_B");

    let err = result.unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg) if msg.contains(
            "TEST_R2E_1472_A -> TEST_R2E_1472_B -> env:TEST_R2E_1472_A"
        )),
        "expected a cycle error naming the chain, got: {err}"
    );
}

#[test]
fn the_same_variable_twice_is_not_a_cycle() {
    let _env = crate::support::env_lock();
    crate::support::set_env("TEST_R2E_1472_PART", "p");
    crate::support::set_env(
        "TEST_R2E_1472_PAIR",
        "${TEST_R2E_1472_PART}-${TEST_R2E_1472_PART}",
    );
    let result = resolve("${TEST_R2E_1472_PAIR}/${TEST_R2E_1472_PAIR}");
    crate::support::remove_env("TEST_R2E_1472_PART");
    crate::support::remove_env("TEST_R2E_1472_PAIR");

    assert_eq!(result.unwrap(), "p-p/p-p");
}

#[test]
fn every_missing_variable_is_named() {
    let _env = crate::support::env_lock();
    for var in ["TEST_R2E_1472_M1", "TEST_R2E_1472_M2", "TEST_R2E_1472_M3"] {
        crate::support::remove_env(var);
    }

    let err = resolve("${TEST_R2E_1472_M2}/${TEST_R2E_1472_M1}/${TEST_R2E_1472_M2}").unwrap_err();
    assert!(
        matches!(&err, ConfigError::NotFound(names)
            if names == "TEST_R2E_1472_M1, TEST_R2E_1472_M2"),
        "got: {err}"
    );

    // Over a whole config: strings, lists and maps report together.
    let mut config = r2e_core::R2eConfig::from_yaml_str(
        r#"
db:
  url: ${TEST_R2E_1472_M1}
  hosts: ["${TEST_R2E_1472_M2}", "${TEST_R2E_1472_M1:ok}"]
  extra:
    token: ${env:TEST_R2E_1472_M3}
"#,
    )
    .unwrap();
    let err = config
        .resolve_placeholders_with(&DefaultSecretResolver)
        .unwrap_err();
    assert!(
        matches!(&err, ConfigError::NotFound(names)
            if names == "TEST_R2E_1472_M1, TEST_R2E_1472_M2, env:TEST_R2E_1472_M3"),
        "got: {err}"
    );
}

#[test]
fn a_single_missing_variable_keeps_its_error() {
    let _env = crate::support::env_lock();
    crate::support::remove_env("TEST_R2E_1472_M1");
    let err = resolve("${TEST_R2E_1472_M1}").unwrap_err();
    assert!(matches!(&err, ConfigError::NotFound(name) if name == "TEST_R2E_1472_M1"));
}

#[test]
fn unclosed_nested_placeholder_is_an_error() {
    let _env = crate::support::env_lock();
    let err = resolve("${TEST_R2E_1472_UNSET:${OTHER}").unwrap_err();
    assert!(matches!(&err, ConfigError::Load(msg) if msg.contains("Unclosed placeholder")));
}
