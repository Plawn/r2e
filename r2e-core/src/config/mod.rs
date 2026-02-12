mod loader;
pub mod registry;
pub mod secrets;
pub mod typed;
pub mod validation;
pub mod value;

use std::collections::HashMap;
use std::path::Path;

pub use secrets::{DefaultSecretResolver, SecretResolver};
pub use registry::{register_section, registered_sections, RegisteredSection};
pub use typed::{ConfigProperties, PropertyMeta};
pub use validation::{validate_keys, validate_section, ConfigValidationError, MissingKeyError};
pub use value::{ConfigValue, FromConfigValue};

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

/// Application configuration loaded from YAML files and environment variables.
///
/// Resolution order:
/// 1. `application.yaml` (base)
/// 2. `application-{profile}.yaml` (profile override)
/// 3. Environment variables (e.g., `APP_DATABASE_URL` overrides `app.database.url`)
///
/// Profile is determined by: `R2E_PROFILE` env var > argument > default `"dev"`.
#[derive(Debug, Clone)]
pub struct R2eConfig {
    values: HashMap<String, ConfigValue>,
    profile: String,
}

impl R2eConfig {
    /// Load configuration for the given profile with a custom secret resolver.
    ///
    /// Looks for `application.yaml` and `application-{profile}.yaml` in the
    /// current working directory, resolves `${...}` placeholders in string values,
    /// then overlays environment variables.
    pub fn load_with_resolver(
        profile: &str,
        resolver: &dyn SecretResolver,
    ) -> Result<Self, ConfigError> {
        let active_profile =
            std::env::var("R2E_PROFILE").unwrap_or_else(|_| profile.to_string());

        let mut values = HashMap::new();

        // 1. Load base config
        loader::load_yaml_file(Path::new("application.yaml"), &mut values)?;

        // 2. Load profile config
        let profile_path = format!("application-{active_profile}.yaml");
        loader::load_yaml_file(Path::new(&profile_path), &mut values)?;

        // 3. Resolve ${...} placeholders in string values
        resolve_string_values(&mut values, resolver)?;

        // 4. Overlay environment variables
        // Convention: `app.database.url` <-> `APP_DATABASE_URL`
        for (env_key, env_val) in std::env::vars() {
            let config_key = env_key.to_lowercase().replace('_', ".");
            values.insert(config_key, ConfigValue::String(env_val));
        }

        Ok(R2eConfig {
            values,
            profile: active_profile,
        })
    }

    /// Load configuration for the given profile (default resolver: env + file).
    ///
    /// Looks for `application.yaml` and `application-{profile}.yaml` in the
    /// current working directory, then overlays environment variables.
    pub fn load(profile: &str) -> Result<Self, ConfigError> {
        Self::load_with_resolver(profile, &DefaultSecretResolver)
    }

    /// Create a config from a YAML string (useful for testing).
    pub fn from_yaml_str(yaml: &str, profile: &str) -> Result<Self, ConfigError> {
        let mut values = HashMap::new();
        loader::load_yaml_str(yaml, &mut values)?;
        Ok(R2eConfig {
            values,
            profile: profile.to_string(),
        })
    }

    /// Create an empty config (useful for testing).
    pub fn empty() -> Self {
        R2eConfig {
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

    /// Check whether a key exists in the config.
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
}

/// Resolve `${...}` placeholders in all string values of the config map.
fn resolve_string_values(
    values: &mut HashMap<String, ConfigValue>,
    resolver: &dyn SecretResolver,
) -> Result<(), ConfigError> {
    let keys: Vec<String> = values.keys().cloned().collect();
    for key in keys {
        if let Some(ConfigValue::String(s)) = values.get(&key) {
            if s.contains("${") {
                let resolved = secrets::resolve_placeholders(s, resolver)?;
                values.insert(key, ConfigValue::String(resolved));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();

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
        let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
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
        let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
        assert_eq!(
            config.get::<String>("app.features.0").unwrap(),
            "openapi"
        );
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
}
