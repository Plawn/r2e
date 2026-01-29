use std::collections::HashMap;
use std::path::Path;

/// Error type for configuration operations.
#[derive(Debug)]
pub enum ConfigError {
    /// The requested key was not found in the configuration.
    NotFound(String),
    /// The value could not be converted to the requested type.
    TypeMismatch { key: String, expected: &'static str },
    /// An I/O or YAML parsing error occurred while loading config files.
    Load(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NotFound(key) => write!(f, "Config key not found: {key}"),
            ConfigError::TypeMismatch { key, expected } => {
                write!(f, "Config type mismatch for '{key}': expected {expected}")
            }
            ConfigError::Load(msg) => write!(f, "Config load error: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A single configuration value that can be converted to various types.
#[derive(Debug, Clone)]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Null,
}

impl ConfigValue {
    fn from_yaml(value: &serde_yaml::Value) -> Self {
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
            other => ConfigValue::String(format!("{other:?}")),
        }
    }
}

/// Trait for converting a `ConfigValue` into a concrete type.
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

/// Application configuration loaded from YAML files and environment variables.
///
/// Resolution order:
/// 1. `application.yaml` (base)
/// 2. `application-{profile}.yaml` (profile override)
/// 3. Environment variables (e.g., `APP_DATABASE_URL` overrides `app.database.url`)
///
/// Profile is determined by: `QUARLUS_PROFILE` env var > argument > default `"dev"`.
#[derive(Debug, Clone)]
pub struct QuarlusConfig {
    values: HashMap<String, ConfigValue>,
    profile: String,
}

/// Load and parse a YAML file, flattening it into the values map.
fn load_yaml_file(path: &Path, values: &mut HashMap<String, ConfigValue>) -> Result<(), ConfigError> {
    if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Load(e.to_string()))?;
        let yaml: serde_yaml::Value = serde_yaml::from_str(&content)
            .map_err(|e| ConfigError::Load(e.to_string()))?;
        flatten_yaml("", &yaml, values);
    }
    Ok(())
}

impl QuarlusConfig {
    /// Load configuration for the given profile.
    ///
    /// Looks for `application.yaml` and `application-{profile}.yaml` in the
    /// current working directory, then overlays environment variables.
    pub fn load(profile: &str) -> Result<Self, ConfigError> {
        let active_profile = std::env::var("QUARLUS_PROFILE").unwrap_or_else(|_| profile.to_string());

        let mut values = HashMap::new();

        // 1. Load base config
        load_yaml_file(Path::new("application.yaml"), &mut values)?;

        // 2. Load profile config
        let profile_path = format!("application-{active_profile}.yaml");
        load_yaml_file(Path::new(&profile_path), &mut values)?;

        // 3. Overlay environment variables
        // Convention: `app.database.url` ↔ `APP_DATABASE_URL`
        for (env_key, env_val) in std::env::vars() {
            let config_key = env_key.to_lowercase().replace('_', ".");
            values.insert(config_key, ConfigValue::String(env_val));
        }

        Ok(QuarlusConfig {
            values,
            profile: active_profile,
        })
    }

    /// Create an empty config (useful for testing).
    pub fn empty() -> Self {
        QuarlusConfig {
            values: HashMap::new(),
            profile: "test".to_string(),
        }
    }

    /// The active profile name.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Get a typed value for the given dot-separated key.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::NotFound` if the key does not exist, or
    /// `ConfigError::TypeMismatch` if the value cannot be converted.
    pub fn get<T: FromConfigValue>(&self, key: &str) -> Result<T, ConfigError> {
        let value = self
            .values
            .get(key)
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))?;
        T::from_config_value(value, key)
    }

    /// Get a typed value, returning a default if the key is missing.
    pub fn get_or<T: FromConfigValue>(&self, key: &str, default: T) -> T {
        self.get(key).unwrap_or(default)
    }

    /// Set a value programmatically.
    pub fn set(&mut self, key: &str, value: ConfigValue) {
        self.values.insert(key.to_string(), value);
    }
}

/// Flatten a YAML tree into dot-separated keys.
fn flatten_yaml(prefix: &str, value: &serde_yaml::Value, out: &mut HashMap<String, ConfigValue>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key_str = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                let full_key = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}.{key_str}")
                };
                flatten_yaml(&full_key, v, out);
            }
        }
        leaf => {
            if !prefix.is_empty() {
                out.insert(prefix.to_string(), ConfigValue::from_yaml(leaf));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config() {
        let config = QuarlusConfig::empty();
        assert!(config.get::<String>("nonexistent").is_err());
    }

    #[test]
    fn test_set_and_get() {
        let mut config = QuarlusConfig::empty();
        config.set("app.name", ConfigValue::String("test".into()));
        assert_eq!(config.get::<String>("app.name").unwrap(), "test");
    }

    #[test]
    fn test_get_or_default() {
        let config = QuarlusConfig::empty();
        assert_eq!(config.get_or("missing", 42i64), 42);
    }

    #[test]
    fn test_type_conversions() {
        let mut config = QuarlusConfig::empty();
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
        let yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
app:
  database:
    url: "sqlite::memory:"
    pool_size: 10
  name: "test"
"#,
        )
        .unwrap();

        let mut values = HashMap::new();
        flatten_yaml("", &yaml, &mut values);

        assert!(matches!(values.get("app.database.url"), Some(ConfigValue::String(s)) if s == "sqlite::memory:"));
        assert!(matches!(values.get("app.database.pool_size"), Some(ConfigValue::Integer(10))));
        assert!(matches!(values.get("app.name"), Some(ConfigValue::String(s)) if s == "test"));
    }
}
