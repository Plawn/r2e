mod loader;
pub mod registry;
pub mod secrets;
pub mod typed;
pub mod validation;
pub mod value;

use std::collections::HashMap;
use std::ops::Deref;
use std::path::Path;

pub use secrets::{DefaultSecretResolver, SecretResolver};
pub use registry::{register_section, registered_sections, RegisteredSection};
pub use typed::{ConfigProperties, PropertyMeta};
pub use validation::{validate_keys, validate_section, ConfigValidationError, MissingKeyError};
pub use value::{ConfigValue, FromConfigValue};

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
/// `R2eConfig` (= `R2eConfig<()>`) provides raw key-value access only.
/// `R2eConfig<T>` adds typed access to a validated config struct via `Deref<Target = T>`.
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
pub struct R2eConfig<T = ()> {
    values: HashMap<String, ConfigValue>,
    profile: String,
    typed: T,
}

// ── Constructors — only on R2eConfig (= R2eConfig<()>) ─────────────────

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
            typed: (),
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
            typed: (),
        })
    }

    /// Create an empty config (useful for testing).
    pub fn empty() -> Self {
        R2eConfig {
            values: HashMap::new(),
            profile: "test".to_string(),
            typed: (),
        }
    }

    /// Set a value programmatically.
    pub fn set(&mut self, key: &str, value: ConfigValue) {
        self.values.insert(key.to_string(), value);
    }

    /// Upgrade to a typed config by constructing `T` from the raw values.
    ///
    /// ```ignore
    /// let config = R2eConfig::load("dev")?.with_typed::<AppConfig>()?;
    /// config.name  // typed field access via Deref
    /// config.get::<String>("app.name")  // raw access still works
    /// ```
    pub fn with_typed<C: ConfigProperties>(self) -> Result<R2eConfig<C>, ConfigError> {
        let typed = C::from_config(&self)?;
        Ok(R2eConfig {
            values: self.values,
            profile: self.profile,
            typed,
        })
    }
}

// ── Methods available on all R2eConfig<T> ───────────────────────────────

impl<T> R2eConfig<T> {
    /// Get a typed value for the given dot-separated key (raw access).
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

    /// Get a typed value, returning a default if the key is missing.
    pub fn get_or<V: FromConfigValue>(&self, key: &str, default: V) -> V {
        self.get(key).unwrap_or(default)
    }

    /// Check whether a key exists in the config.
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// The active profile name.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Get a reference to the typed config layer.
    pub fn typed(&self) -> &T {
        &self.typed
    }

    /// Downgrade to a raw (untyped) config, discarding the typed layer.
    pub fn raw(&self) -> R2eConfig {
        R2eConfig {
            values: self.values.clone(),
            profile: self.profile.clone(),
            typed: (),
        }
    }
}

// ── Deref for ergonomic typed field access ──────────────────────────────

impl<T> Deref for R2eConfig<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.typed
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
