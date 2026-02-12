use std::collections::HashMap;

use super::ConfigError;

/// A single configuration value that can be converted to various types.
#[derive(Debug, Clone)]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Null,
    List(Vec<ConfigValue>),
    Map(HashMap<String, ConfigValue>),
}

impl ConfigValue {
    pub(crate) fn from_yaml(value: &serde_yaml::Value) -> Self {
        match value {
            serde_yaml::Value::Bool(b) => ConfigValue::Bool(*b),
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    ConfigValue::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    ConfigValue::Float(f)
                } else {
                    ConfigValue::String(n.to_string())
                }
            }
            serde_yaml::Value::String(s) => ConfigValue::String(s.clone()),
            serde_yaml::Value::Null => ConfigValue::Null,
            serde_yaml::Value::Sequence(seq) => {
                ConfigValue::List(seq.iter().map(ConfigValue::from_yaml).collect())
            }
            serde_yaml::Value::Mapping(map) => {
                let mut result = HashMap::new();
                for (k, v) in map {
                    let key = match k {
                        serde_yaml::Value::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    result.insert(key, ConfigValue::from_yaml(v));
                }
                ConfigValue::Map(result)
            }
            other => ConfigValue::String(format!("{other:?}")),
        }
    }
}

/// Trait for converting a `ConfigValue` into a concrete type.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be used as a config value type",
    label = "not a valid config value type",
    note = "built-in types: String, i64, f64, bool, Option<T>, Vec<T>. Implement `FromConfigValue` for custom types."
)]
pub trait FromConfigValue: Sized {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError>;
}

impl FromConfigValue for String {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::String(s) => Ok(s.clone()),
            ConfigValue::Integer(i) => Ok(i.to_string()),
            ConfigValue::Float(f) => Ok(f.to_string()),
            ConfigValue::Bool(b) => Ok(b.to_string()),
            ConfigValue::Null => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "String",
            }),
            ConfigValue::List(_) => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "String",
            }),
            ConfigValue::Map(_) => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "String",
            }),
        }
    }
}

impl FromConfigValue for i64 {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::Integer(i) => Ok(*i),
            ConfigValue::String(s) => s.parse().map_err(|_| ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "i64",
            }),
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "i64",
            }),
        }
    }
}

impl FromConfigValue for f64 {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::Float(f) => Ok(*f),
            ConfigValue::Integer(i) => Ok(*i as f64),
            ConfigValue::String(s) => s.parse().map_err(|_| ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "f64",
            }),
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "f64",
            }),
        }
    }
}

impl FromConfigValue for bool {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::Bool(b) => Ok(*b),
            ConfigValue::String(s) => match s.to_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(true),
                "false" | "0" | "no" => Ok(false),
                _ => Err(ConfigError::TypeMismatch {
                    key: key.to_string(),
                    expected: "bool",
                }),
            },
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "bool",
            }),
        }
    }
}

impl<T: FromConfigValue> FromConfigValue for Option<T> {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::Null => Ok(None),
            v => T::from_config_value(v, key).map(Some),
        }
    }
}

impl<T: FromConfigValue> FromConfigValue for Vec<T> {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::List(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| T::from_config_value(v, &format!("{key}[{i}]")))
                .collect(),
            // Fallback: single value -> vec of one
            other => Ok(vec![T::from_config_value(other, key)?]),
        }
    }
}
