mod loader;
pub mod registry;
pub mod secrets;
pub mod typed;
pub mod validation;
pub mod value;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub use secrets::{DefaultSecretResolver, SecretResolver};
pub use registry::{register_section, registered_sections, RegisteredSection};
pub use typed::{ConfigProperties, NoChildren, PropertyMeta};
pub use validation::{
    validate_keys, validate_section, validate_section_keys, ConfigValidationError,
    MissingKeyError,
};
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
/// 3. Environment variables prefixed with `R2E_` (e.g., `R2E_SERVER_PORT=8080`
///    sets `server.port`). The prefix is stripped, the remainder is
///    lowercased, and `_` is replaced with `.`. Unprefixed env vars are
///    ignored so the config namespace does not collide with the general
///    process environment.
///
/// `.env` files never overwrite already-set environment variables.
#[derive(Debug, Clone)]
pub struct R2eConfig {
    values: Arc<HashMap<String, ConfigValue>>,
}

/// Default base config file name used by [`R2eConfig::load`].
pub const DEFAULT_CONFIG_FILE: &str = "application.yaml";

/// Derive the profile overlay file for a base config file:
/// `<stem>-<profile>.<ext>` next to the base file (`patina.yaml` + `test`
/// → `patina-test.yaml`; an extension-less base gets `-<profile>` appended).
fn profile_file_for(base: &Path, profile: &str) -> std::path::PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let name = match base.extension() {
        Some(ext) => format!("{stem}-{profile}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{profile}"),
    };
    base.with_file_name(name)
}

/// Overlay `R2E_`-prefixed environment variables onto a config values map.
fn apply_env_overlay<I: IntoIterator<Item = (String, String)>>(
    values: &mut HashMap<String, ConfigValue>,
    env: I,
) {
    for (env_key, env_val) in env {
        if let Some(rest) = env_key.strip_prefix("R2E_") {
            if rest.is_empty() {
                continue;
            }
            let config_key = rest.to_lowercase().replace('_', ".");
            values.insert(config_key, ConfigValue::String(env_val));
        }
    }
}

// ── Constructors ─────────────────────────────────────────────────────────

impl R2eConfig {
    /// Load configuration with a custom secret resolver.
    ///
    /// Looks for `application.yaml` in the current working directory,
    /// overlays the profile file (see
    /// [`load_profiled_with_resolver`](Self::load_profiled_with_resolver)),
    /// resolves `${...}` placeholders in string values, then overlays
    /// environment variables.
    pub fn load_with_resolver(
        resolver: &dyn SecretResolver,
    ) -> Result<Self, ConfigError> {
        Self::load_profiled_with_resolver(None, resolver)
    }

    /// Load configuration for a specific profile.
    ///
    /// Same pipeline as
    /// [`load_from_with_resolver`](Self::load_from_with_resolver), with the
    /// default `application.yaml` base file — except a missing base file is
    /// tolerated here (apps may run on env vars alone).
    pub fn load_profiled_with_resolver(
        profile: Option<&str>,
        resolver: &dyn SecretResolver,
    ) -> Result<Self, ConfigError> {
        // The default base file is optional: apps may run on env vars alone.
        Self::load_impl(Path::new(DEFAULT_CONFIG_FILE), profile, resolver, false)
    }

    /// Load configuration from a custom base file, for a specific profile,
    /// with a custom secret resolver. This is the full form every other
    /// `load*` constructor delegates to.
    ///
    /// Resolution order (lowest to highest priority):
    /// 1. The base file (e.g. `patina.yaml`)
    /// 2. Its profile sibling `<stem>-{profile}.<ext>` (e.g.
    ///    `patina-test.yaml`, when the file exists and the profile is not
    ///    `default`)
    /// 3. `${...}` secret placeholders resolved in string values
    /// 4. `R2E_`-prefixed environment variables
    ///
    /// The profile itself is resolved as: `profile` argument >
    /// `R2E_PROFILE` env var > `r2e.profile` key in the base file >
    /// `"default"`. The resolved profile is written back to the
    /// `r2e.profile` key so downstream consumers agree on it.
    ///
    /// # Errors
    ///
    /// Unlike [`load`](Self::load) — where the default `application.yaml` is
    /// optional — an explicitly requested base file that does not exist is
    /// `ConfigError::Load` (a typo'd name would otherwise silently yield an
    /// empty config). The profile sibling stays optional.
    pub fn load_from_with_resolver(
        file: impl AsRef<Path>,
        profile: Option<&str>,
        resolver: &dyn SecretResolver,
    ) -> Result<Self, ConfigError> {
        Self::load_impl(file.as_ref(), profile, resolver, true)
    }

    fn load_impl(
        file: &Path,
        profile: Option<&str>,
        resolver: &dyn SecretResolver,
        require_file: bool,
    ) -> Result<Self, ConfigError> {
        let mut values = HashMap::new();

        // 1. Load base config. `is_file()` (not `exists()`) so a directory
        // path gets this clear error instead of a raw "Is a directory" io
        // error from the read.
        if require_file && !file.is_file() {
            return Err(ConfigError::Load(format!(
                "config file not found (or not a regular file): {}",
                file.display()
            )));
        }
        loader::load_yaml_file(file, &mut values)?;

        // 2. Load .env file (does NOT overwrite existing env vars)
        let _ = dotenvy::dotenv();

        // 3. Overlay the profile-specific file.
        let resolved_profile = profile
            .map(str::to_string)
            .or_else(|| std::env::var("R2E_PROFILE").ok())
            .or_else(|| match values.get("r2e.profile") {
                Some(ConfigValue::String(s)) => Some(s.clone()),
                _ => None,
            });
        if let Some(p) = resolved_profile {
            if p != "default" {
                loader::load_yaml_file(&profile_file_for(file, &p), &mut values)?;
            }
            values.insert("r2e.profile".to_string(), ConfigValue::String(p));
        }

        // 4. Resolve ${...} placeholders in string values
        resolve_string_values(&mut values, resolver)?;

        // 5. Overlay environment variables.
        apply_env_overlay(&mut values, std::env::vars());

        Ok(R2eConfig {
            values: Arc::new(values),
        })
    }

    /// Load configuration (default resolver: env + file).
    ///
    /// Looks for `application.yaml` in the current working directory,
    /// overlays `application-{profile}.yaml` when a profile is active,
    /// then overlays environment variables.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_resolver(&DefaultSecretResolver)
    }

    /// [`load`](Self::load) with an explicit profile (wins over `R2E_PROFILE`).
    pub fn load_profiled(profile: Option<&str>) -> Result<Self, ConfigError> {
        Self::load_profiled_with_resolver(profile, &DefaultSecretResolver)
    }

    /// Load configuration from a custom base file instead of
    /// `application.yaml` (e.g. `R2eConfig::load_from("patina.yaml")`).
    ///
    /// The profile overlay file is derived from the base name
    /// (`patina.yaml` + profile `test` → `patina-test.yaml`); secret
    /// resolution and the env overlay apply exactly as in
    /// [`load`](Self::load). A missing base file is an error (see
    /// [`load_from_with_resolver`](Self::load_from_with_resolver)).
    pub fn load_from(file: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Self::load_from_with_resolver(file, None, &DefaultSecretResolver)
    }

    /// [`load_from`](Self::load_from) with an explicit profile (wins over
    /// `R2E_PROFILE`).
    pub fn load_profiled_from(
        file: impl AsRef<Path>,
        profile: Option<&str>,
    ) -> Result<Self, ConfigError> {
        Self::load_from_with_resolver(file, profile, &DefaultSecretResolver)
    }

    /// Apply the `R2E_`-prefixed environment overlay to an existing values
    /// map. Exposed for testing the overlay behaviour without mutating the
    /// process environment.
    #[doc(hidden)]
    pub fn apply_env_overlay_for_test<I: IntoIterator<Item = (String, String)>>(
        values: &mut HashMap<String, ConfigValue>,
        env: I,
    ) {
        apply_env_overlay(values, env);
    }

    /// Create a config from a YAML string (useful for testing).
    pub fn from_yaml_str(yaml: &str) -> Result<Self, ConfigError> {
        let mut values = HashMap::new();
        loader::load_yaml_str(yaml, &mut values)?;
        Ok(R2eConfig {
            values: Arc::new(values),
        })
    }

    /// Create an empty config (useful for testing).
    pub fn empty() -> Self {
        R2eConfig {
            values: Arc::new(HashMap::new()),
        }
    }

    /// Set a value programmatically.
    pub fn set(&mut self, key: &str, value: ConfigValue) {
        Arc::make_mut(&mut self.values).insert(key.to_string(), value);
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

    /// Resolve an **optional** typed value: `Ok(None)` when the key is absent or
    /// explicitly `null`, `Ok(Some(v))` when present, `Err` on a type mismatch.
    ///
    /// This is the primitive behind `#[config("key")] x: Option<T>`. Unlike
    /// [`try_get`](Self::try_get) — which is fail-open and swallows a type
    /// mismatch into `None` — `get_opt` reports a mistyped value as an error so
    /// the caller can fail loudly. Absence is the only condition that maps to
    /// `None`.
    pub fn get_opt<V: FromConfigValue>(&self, key: &str) -> Result<Option<V>, ConfigError> {
        match self.values.get(key) {
            None | Some(ConfigValue::Null) => Ok(None),
            Some(value) => V::from_config_value(value, key).map(Some),
        }
    }

    /// Get a typed value, returning a default if the key is missing.
    pub fn get_or<V: FromConfigValue>(&self, key: &str, default: V) -> V {
        self.get(key).unwrap_or(default)
    }

    /// Check whether a key exists in the config.
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Check whether any key lives under the given prefix.
    ///
    /// Returns `true` when a key equal to `prefix` holds a non-null value or
    /// any key starts with `"{prefix}."`. This is how section presence is
    /// detected: a section is "present" when it contributes at least one
    /// key, whether from YAML or from the `R2E_` env overlay. An empty YAML
    /// section (`tls:` with no content, flattened to a `Null` at the exact
    /// key) counts as absent.
    pub fn has_prefix(&self, prefix: &str) -> bool {
        match self.values.get(prefix) {
            Some(ConfigValue::Null) | None => {}
            Some(_) => return true,
        }
        let dotted = format!("{prefix}.");
        self.values.keys().any(|k| k.starts_with(&dotted))
    }

    /// List the distinct immediate child segments under a prefix, sorted.
    ///
    /// For keys `upstreams.npm.url` and `upstreams.docker.url` with prefix
    /// `"upstreams"`, returns `["docker", "npm"]`. Keys equal to the prefix
    /// itself are ignored. This is the enumeration primitive for map-shaped
    /// config sections.
    pub fn sub_keys(&self, prefix: &str) -> Vec<String> {
        let dotted = format!("{prefix}.");
        let mut segments: Vec<String> = self
            .values
            .keys()
            .filter_map(|k| k.strip_prefix(&dotted))
            .map(|rest| match rest.find('.') {
                Some(i) => rest[..i].to_string(),
                None => rest.to_string(),
            })
            .collect();
        segments.sort();
        segments.dedup();
        segments
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

    fn register(_config: &R2eConfig, registry: &mut crate::beans::BeanRegistry) -> Result<(), ConfigError> {
        // `load_config::<C>()` unconditionally pushes `C` onto the
        // compile-time provision list, so the unit slot must exist in the
        // registry or `build_state()` fails with "Bean of type `()` not found".
        registry.provide(());
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

// ── PluginConfig trait ───────────────────────────────────────────────────

/// The typed-configuration surface of a [`PreStatePlugin`](crate::PreStatePlugin).
///
/// A plugin names a `Config` type (its [`PreStatePlugin::Config`](crate::PreStatePlugin::Config))
/// plus a section prefix
/// ([`PreStatePlugin::CONFIG_PREFIX`](crate::PreStatePlugin::CONFIG_PREFIX)). At
/// `configure` time — after `build_state()`, when [`R2eConfig`] is guaranteed
/// loaded — the framework loads and validates that section and hands the plugin
/// an `Option<Config>` (see the loading rules on
/// [`PreStatePlugin::configure`](crate::PreStatePlugin::configure)).
///
/// This trait is implemented for two shapes, mirroring [`LoadableConfig`]:
///
/// - `()` — the "no config" case. `type Config = ();` (the default surface);
///   loading always yields nothing and validation always passes.
/// - any `T: ConfigProperties` — the section is read via
///   [`ConfigProperties::from_config`] and validated via
///   [`validate_section`](crate::config::validate_section), so a plugin section
///   produces exactly the same missing-key / type-mismatch boot errors a
///   controller's `#[config(section)]` does.
///
/// Because the `ConfigProperties` impl is blanket, plugin authors never write a
/// `PluginConfig` impl by hand: a `#[derive(ConfigProperties)]` struct is a
/// valid `type Config` out of the box.
pub trait PluginConfig: Send + Sized + 'static {
    /// Construct the config from the loaded [`R2eConfig`] at the given prefix.
    fn plugin_load(config: &R2eConfig, prefix: &str) -> Result<Self, ConfigError>;

    /// Validate the section at `prefix` — the same missing-key / type-mismatch
    /// walk controllers use. An empty vec means the section is valid.
    fn plugin_validate(config: &R2eConfig, prefix: &str) -> Vec<MissingKeyError>;
}

impl PluginConfig for () {
    fn plugin_load(_config: &R2eConfig, _prefix: &str) -> Result<(), ConfigError> {
        Ok(())
    }

    fn plugin_validate(_config: &R2eConfig, _prefix: &str) -> Vec<MissingKeyError> {
        Vec::new()
    }
}

impl<T: ConfigProperties + Send + 'static> PluginConfig for T {
    fn plugin_load(config: &R2eConfig, prefix: &str) -> Result<T, ConfigError> {
        T::from_config(config, Some(prefix))
    }

    fn plugin_validate(config: &R2eConfig, prefix: &str) -> Vec<MissingKeyError> {
        validate_section::<T>(config, Some(prefix))
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
    use super::{ConfigValue, R2eConfig};
    use std::sync::Arc;

    #[test]
    fn clone_shares_values_backing_store() {
        let config = R2eConfig::from_yaml_str("app:\n  name: original\n").unwrap();
        let cloned = config.clone();

        assert!(Arc::ptr_eq(&config.values, &cloned.values));
    }

    #[test]
    fn set_on_clone_uses_copy_on_write() {
        let config = R2eConfig::from_yaml_str("app:\n  name: original\n").unwrap();
        let mut cloned = config.clone();

        cloned.set("app.name", ConfigValue::String("updated".into()));

        assert!(!Arc::ptr_eq(&config.values, &cloned.values));
        assert_eq!(config.get::<String>("app.name").unwrap(), "original");
        assert_eq!(cloned.get::<String>("app.name").unwrap(), "updated");
    }
}
