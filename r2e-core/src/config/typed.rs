use super::{ConfigError, R2eConfig};

/// Metadata about a single configuration property.
#[derive(Debug, Clone)]
pub struct PropertyMeta {
    /// Relative key (e.g., `"pool_size"`).
    pub key: String,
    /// Absolute key (e.g., `"app.database.pool_size"`).
    pub full_key: String,
    /// Rust type name (e.g., `"i64"`).
    pub type_name: &'static str,
    /// Whether the property is required (no default and not `Option`).
    pub required: bool,
    /// Default value as a string, if any.
    pub default_value: Option<String>,
    /// Description from doc comments.
    pub description: Option<String>,
}

/// Trait for strongly-typed configuration sections.
///
/// Implement via `#[derive(ConfigProperties)]`:
///
/// ```ignore
/// #[derive(ConfigProperties, Clone, Debug)]
/// #[config(prefix = "app.database")]
/// pub struct DatabaseConfig {
///     /// Database connection URL
///     pub url: String,
///
///     /// Connection pool size (default: 10)
///     #[config(default = 10)]
///     pub pool_size: i64,
///
///     /// Optional connection timeout in seconds
///     pub timeout: Option<i64>,
/// }
/// ```
pub trait ConfigProperties: Sized {
    /// The configuration key prefix (e.g., `"app.database"`).
    fn prefix() -> &'static str;

    /// Metadata about all expected properties.
    fn properties_metadata() -> Vec<PropertyMeta>;

    /// Construct from an `R2eConfig` instance.
    fn from_config(config: &R2eConfig) -> Result<Self, ConfigError>;
}
