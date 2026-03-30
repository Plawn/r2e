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
pub use value::{ConfigValue, FromConfigValue, deserialize_value};

/// A single validation error detail from typed config validation (e.g., garde).
#[derive(Debug, Clone)]
pub struct ConfigValidationDetail {
    pub key: String,
    pub message: String,
}

/// Error type for configuration operations.
#[derive(Debug)]
pub enum ConfigError {
    /// The requested key was not found in the configuration.
    NotFound(String),
    /// The value could not be converted to the requested type.
    TypeMismatch { key: String, expected: &'static str },
    /// An I/O or YAML parsing error occurred while loading config files.
    Load(String),
    /// Serde deserialization failed (used by `deserialize_value` / `FromConfigValue` derive).
    Deserialize { key: String, message: String },
    /// Validation errors from typed config (e.g., garde constraints).
    Validation(Vec<ConfigValidationDetail>),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NotFound(key) => write!(f, "Config key not found: {key}"),
            ConfigError::TypeMismatch { key, expected } => {
                write!(f, "Config type mismatch for '{key}': expected {expected}")
            }
            ConfigError::Load(msg) => write!(f, "Config load error: {msg}"),
            ConfigError::Deserialize { key, message } => {
                write!(f, "Config deserialization error for '{key}': {message}")
            }
            ConfigError::Validation(details) => {
                write!(f, "Config validation errors:")?;
                for detail in details {
                    write!(f, "\n  - {}: {}", detail.key, detail.message)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Application configuration loaded from YAML files, `.env` files, and environment variables.
///
/// Resolution order (lowest to highest priority):
/// 1. `application.yaml`
/// 2. `.env` file (loaded into process environment)
/// 3. Environment variables (e.g., `APP_DATABASE_URL` overrides `app.database.url`)
///
/// `.env` files never overwrite already-set environment variables.
#[derive(Debug, Clone)]
pub struct R2eConfig {
    values: HashMap<String, ConfigValue>,
}

// ── Constructors ─────────────────────────────────────────────────────────

impl R2eConfig {
    /// Load configuration with a custom secret resolver.
    ///
    /// Looks for `application.yaml` in the current working directory,
    /// resolves `${...}` placeholders in string values, then overlays
    /// environment variables.
    pub fn load_with_resolver(
        resolver: &dyn SecretResolver,
    ) -> Result<Self, ConfigError> {
        let mut values = HashMap::new();

        // 1. Load base config
        loader::load_yaml_file(Path::new("application.yaml"), &mut values)?;

        // 2. Load .env file (does NOT overwrite existing env vars)
        let _ = dotenvy::dotenv();

        // 3. Resolve ${...} placeholders in string values
        resolve_string_values(&mut values, resolver)?;

        // 4. Overlay environment variables
        // Convention: `app.database.url` <-> `APP_DATABASE_URL`
        for (env_key, env_val) in std::env::vars() {
            let config_key = env_key.to_lowercase().replace('_', ".");
            values.insert(config_key, ConfigValue::String(env_val));
        }

        Ok(R2eConfig { values })
    }

    /// Load configuration (default resolver: env + file).
    ///
    /// Looks for `application.yaml` in the current working directory,
    /// then overlays environment variables.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_resolver(&DefaultSecretResolver)
    }

    /// Create a config from a YAML string (useful for testing).
    pub fn from_yaml_str(yaml: &str) -> Result<Self, ConfigError> {
        let mut values = HashMap::new();
        loader::load_yaml_str(yaml, &mut values)?;
        Ok(R2eConfig { values })
    }

    /// Create an empty config (useful for testing).
    pub fn empty() -> Self {
        R2eConfig {
            values: HashMap::new(),
        }
    }

    /// Set a value programmatically.
    pub fn set(&mut self, key: &str, value: ConfigValue) {
        self.values.insert(key.to_string(), value);
    }
}

// ── Methods ──────────────────────────────────────────────────────────────

impl R2eConfig {
    /// Get a typed value for the given dot-separated key.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::NotFound` if the key does not exist, or
    /// `ConfigError::TypeMismatch` if the value cannot be converted.
    pub fn get<V: FromConfigValue>(&self, key: &str) -> Result<V, ConfigError> {
        let value = self
            .values
            .get(key)
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))?;
        V::from_config_value(value, key)
    }

    /// Try to get a typed value, returning `None` if the key is missing.
    ///
    /// Unlike [`get`](Self::get), this does not return an error on missing keys.
    /// Type mismatches still return `None`.
    pub fn try_get<V: FromConfigValue>(&self, key: &str) -> Option<V> {
        self.get(key).ok()
    }

    /// Get a typed value, returning a default if the key is missing.
    pub fn get_or<V: FromConfigValue>(&self, key: &str, default: V) -> V {
        self.get(key).unwrap_or(default)
    }

    /// Check whether a key exists in the config.
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Compute a fingerprint (hash) over a set of config keys.
    ///
    /// Returns a `u64` hash of the values at the given keys. Used by the
    /// dev-reload bean cache to detect config changes.
    pub fn config_fingerprint(&self, keys: &[&str]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for &key in keys {
            key.hash(&mut hasher);
            match self.values.get(key) {
                Some(v) => {
                    1u8.hash(&mut hasher);
                    v.hash(&mut hasher);
                }
                None => {
                    0u8.hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }

}

// ── LoadableConfig trait ─────────────────────────────────────────────────

/// Trait that enables `AppBuilder::load_config::<C>()`.
///
/// Implemented for `()` (raw config only) and for any `T: ConfigProperties`
/// (raw config + typed bean).
pub trait LoadableConfig: Clone + Send + Sync + 'static {
    /// Type-level list of all child config types registered by this config.
    ///
    /// For `()`, this is `TNil`. For `T: ConfigProperties`, this is `T::Children`.
    /// Used by `load_config` to add children to the compile-time provision list.
    type Children;

    /// Optionally register additional beans derived from the config.
    fn register(config: &R2eConfig, registry: &mut crate::beans::BeanRegistry) -> Result<(), ConfigError>;
}

impl LoadableConfig for () {
    type Children = crate::type_list::TNil;

    fn register(_config: &R2eConfig, _registry: &mut crate::beans::BeanRegistry) -> Result<(), ConfigError> {
        Ok(())
    }
}

impl<T: ConfigProperties + Clone + Send + Sync + 'static> LoadableConfig for T {
    type Children = T::Children;

    fn register(config: &R2eConfig, registry: &mut crate::beans::BeanRegistry) -> Result<(), ConfigError> {
        let typed = T::from_config(config, None)?;
        typed.register_children(registry);
        registry.provide(typed);
        Ok(())
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
