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

/// Application configuration loaded from YAML files, `.env` files, and environment variables.
///
/// Resolution order (lowest to highest priority):
/// 1. `application.yaml` (base)
/// 2. `application-{profile}.yaml` (profile override)
/// 3. `.env` file (loaded into process environment)
/// 4. `.env.{profile}` file (loaded into process environment)
/// 5. Environment variables (e.g., `APP_DATABASE_URL` overrides `app.database.url`)
///
/// `.env` files never overwrite already-set environment variables.
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

        // 3. Load .env files (does NOT overwrite existing env vars)
        let _ = dotenvy::dotenv();
        let profile_env = format!(".env.{active_profile}");
        let _ = dotenvy::from_filename(&profile_env);

        // 4. Resolve ${...} placeholders in string values
        resolve_string_values(&mut values, resolver)?;

        // 5. Overlay environment variables
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
