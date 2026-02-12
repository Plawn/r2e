use super::typed::ConfigProperties;
use super::{ConfigError, R2eConfig};

/// A single missing config key.
#[derive(Debug)]
pub struct MissingKeyError {
    /// Source that requires this key (bean name, controller name, section prefix).
    pub source: String,
    /// The config key that is missing.
    pub key: String,
    /// The expected type name.
    pub expected_type: String,
    /// Environment variable hint.
    pub env_hint: String,
    /// Optional description (from `ConfigProperties` metadata).
    pub description: Option<String>,
}

impl std::fmt::Display for MissingKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  - `{}`: key '{}' ({}) — set env var `{}`",
            self.source, self.key, self.expected_type, self.env_hint
        )?;
        if let Some(desc) = &self.description {
            write!(f, " -- {}", desc)?;
        }
        Ok(())
    }
}

/// Aggregated config validation error.
#[derive(Debug)]
pub struct ConfigValidationError {
    pub errors: Vec<MissingKeyError>,
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Missing configuration keys:")?;
        for err in &self.errors {
            writeln!(f, "{}", err)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigValidationError {}

/// Validate a list of config keys against an `R2eConfig`.
///
/// Each entry is `(source_name, key, type_name)`. Returns the list of
/// missing keys as [`MissingKeyError`]s (empty if all present).
pub fn validate_keys(
    config: &R2eConfig,
    keys: &[(&str, &str, &str)],
) -> Vec<MissingKeyError> {
    keys.iter()
        .filter(|(_, key, _)| !config.contains_key(key))
        .map(|(source, key, type_name)| MissingKeyError {
            source: source.to_string(),
            key: key.to_string(),
            expected_type: type_name.to_string(),
            env_hint: key.to_uppercase().replace('.', "_"),
            description: None,
        })
        .collect()
}

/// Validate a `ConfigProperties` section against an `R2eConfig`.
///
/// Checks that all required keys are present. Returns a list of
/// missing keys (empty if all present).
pub fn validate_section<C: ConfigProperties>(
    config: &R2eConfig,
) -> Vec<MissingKeyError> {
    let meta = C::properties_metadata();
    let prefix = C::prefix();

    meta.iter()
        .filter(|prop| prop.required)
        .filter(|prop| matches!(config.get::<String>(&prop.full_key), Err(ConfigError::NotFound(_))))
        .map(|prop| MissingKeyError {
            source: prefix.to_string(),
            key: prop.full_key.clone(),
            expected_type: prop.type_name.to_string(),
            env_hint: prop.full_key.to_uppercase().replace('.', "_"),
            description: prop.description.clone(),
        })
        .collect()
}
