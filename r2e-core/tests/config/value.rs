//! `FromConfigValue`: built-in scalar conversions and the derive (enums).

use r2e_core::config::{ConfigError, ConfigProperties, ConfigValue, R2eConfig};

// =========================================================================
// FromConfigValue — numeric types
// =========================================================================

#[test]
fn test_from_config_value_u16() {
    let mut config = R2eConfig::empty();
    config.set("port", ConfigValue::Integer(8080));
    assert_eq!(config.get::<u16>("port").unwrap(), 8080);
}

#[test]
fn test_from_config_value_u32() {
    let mut config = R2eConfig::empty();
    config.set("count", ConfigValue::Integer(100_000));
    assert_eq!(config.get::<u32>("count").unwrap(), 100_000);
}

#[test]
fn test_from_config_value_u8() {
    let mut config = R2eConfig::empty();
    config.set("level", ConfigValue::Integer(255));
    assert_eq!(config.get::<u8>("level").unwrap(), 255);
}

#[test]
fn test_from_config_value_u8_out_of_range() {
    let mut config = R2eConfig::empty();
    config.set("level", ConfigValue::Integer(256));
    assert!(config.get::<u8>("level").is_err());
}

#[test]
fn test_from_config_value_i32() {
    let mut config = R2eConfig::empty();
    config.set("val", ConfigValue::Integer(-42));
    assert_eq!(config.get::<i32>("val").unwrap(), -42);
}

#[test]
fn test_from_config_value_usize() {
    let mut config = R2eConfig::empty();
    config.set("size", ConfigValue::Integer(1024));
    assert_eq!(config.get::<usize>("size").unwrap(), 1024);
}

#[test]
fn test_from_config_value_f32() {
    let mut config = R2eConfig::empty();
    config.set("ratio", ConfigValue::Float(1.5));
    let val = config.get::<f32>("ratio").unwrap();
    assert!((val - 1.5).abs() < f32::EPSILON);
}

#[test]
fn test_from_config_value_hashmap() {
    use std::collections::HashMap;
    let mut inner = HashMap::new();
    inner.insert("env".to_string(), ConfigValue::String("production".into()));
    inner.insert("region".to_string(), ConfigValue::String("us-east".into()));
    let mut config = R2eConfig::empty();
    config.set("labels", ConfigValue::Map(inner));

    let labels: HashMap<String, String> = config.get("labels").unwrap();
    assert_eq!(labels.get("env").unwrap(), "production");
    assert_eq!(labels.get("region").unwrap(), "us-east");
}

// =========================================================================
// Feature 2: FromConfigValue derive — enum support via serde
// =========================================================================

#[derive(serde::Deserialize, r2e_macros::FromConfigValue, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum AppMode {
    Development,
    Production,
    Staging,
}

#[test]
fn test_from_config_value_enum() {
    let mut config = R2eConfig::empty();
    config.set("mode", ConfigValue::String("production".into()));
    let mode: AppMode = config.get("mode").unwrap();
    assert_eq!(mode, AppMode::Production);
}

#[test]
fn test_from_config_value_enum_invalid() {
    let mut config = R2eConfig::empty();
    config.set("mode", ConfigValue::String("invalid".into()));
    let result = config.get::<AppMode>("mode");
    assert!(result.is_err());
    match result.unwrap_err() {
        ConfigError::Deserialize { key, message } => {
            assert_eq!(key, "mode");
            assert!(message.contains("unknown variant"));
        }
        other => panic!("Expected Deserialize error, got: {other:?}"),
    }
}

#[test]
fn test_from_config_value_enum_option() {
    let config = R2eConfig::empty();
    let result: Option<AppMode> = config.get_or("mode", None);
    assert!(result.is_none());

    let mut config2 = R2eConfig::empty();
    config2.set("mode", ConfigValue::String("staging".into()));
    let result2: Option<AppMode> = config2.get("mode").unwrap();
    assert_eq!(result2, Some(AppMode::Staging));
}

// Enum as field in ConfigProperties struct
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct AppWithMode {
    pub name: String,
    pub mode: Option<AppMode>,
}

#[test]
fn test_enum_in_config_properties() {
    let yaml = r#"
app:
  name: "my-app"
  mode: "production"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppWithMode::from_config(&config, Some("app")).unwrap();
    assert_eq!(app.name, "my-app");
    assert_eq!(app.mode, Some(AppMode::Production));
}

#[test]
fn test_enum_in_config_properties_absent() {
    let yaml = r#"
app:
  name: "my-app"
"#;
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    let app = AppWithMode::from_config(&config, Some("app")).unwrap();
    assert_eq!(app.mode, None);
}

// General case: struct with #[derive(Deserialize, FromConfigValue)]
#[derive(serde::Deserialize, r2e_macros::FromConfigValue, Clone, Debug, PartialEq)]
struct Endpoint {
    pub host: String,
    pub port: u16,
}

#[test]
fn test_from_config_value_struct() {
    use std::collections::HashMap;
    let mut inner = HashMap::new();
    inner.insert("host".to_string(), ConfigValue::String("localhost".into()));
    inner.insert("port".to_string(), ConfigValue::Integer(8080));

    let mut config = R2eConfig::empty();
    config.set("endpoint", ConfigValue::Map(inner));

    let ep: Endpoint = config.get("endpoint").unwrap();
    assert_eq!(ep.host, "localhost");
    assert_eq!(ep.port, 8080);
}
