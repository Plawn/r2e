//! `R2eConfig` itself: get/set, typed reads, prefix helpers.

use r2e_core::config::{ConfigValue, R2eConfig};

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
fn test_get_opt() {
    let mut config = R2eConfig::empty();
    config.set("present", ConfigValue::Integer(7));
    config.set("null_val", ConfigValue::Null);
    config.set("wrong_type", ConfigValue::String("not-an-int".into()));

    // Absent → Ok(None); explicit null → Ok(None); present → Ok(Some(v)).
    assert_eq!(config.get_opt::<i64>("missing").unwrap(), None);
    assert_eq!(config.get_opt::<i64>("null_val").unwrap(), None);
    assert_eq!(config.get_opt::<i64>("present").unwrap(), Some(7));

    // Unlike `try_get` (fail-open → None), a type mismatch is a loud error.
    assert!(config.get_opt::<i64>("wrong_type").is_err());
    assert_eq!(config.try_get::<i64>("wrong_type"), None);
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
    let config = R2eConfig::from_yaml_str(yaml).unwrap();

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
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
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
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert_eq!(config.get::<String>("app.features.0").unwrap(), "openapi");
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

// =========================================================================
// Prefix helpers: has_prefix / sub_keys
// =========================================================================

#[test]
fn test_has_prefix() {
    let yaml = r#"
app:
  database:
    url: "sqlite::memory:"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert!(config.has_prefix("app"));
    assert!(config.has_prefix("app.database"));
    assert!(config.has_prefix("app.database.url")); // exact key counts
    assert!(!config.has_prefix("app.data")); // not a segment boundary
    assert!(!config.has_prefix("missing"));
}

#[test]
fn test_sub_keys() {
    let yaml = r#"
upstreams:
  npm:
    url: "https://registry.npmjs.org"
  docker:
    url: "https://registry-1.docker.io"
    auth:
      token: "x"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    assert_eq!(config.sub_keys("upstreams"), vec!["docker", "npm"]);
    assert_eq!(config.sub_keys("upstreams.docker"), vec!["auth", "url"]);
    assert!(config.sub_keys("missing").is_empty());
}
