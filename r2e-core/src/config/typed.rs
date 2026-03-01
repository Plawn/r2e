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
///     pub url: String,
///     #[config(default = 10)]
///     pub pool_size: i64,
///     pub timeout: Option<i64>,
/// }
///
/// let config = R2eConfig::from_yaml_str("app:\n  database:\n    url: postgres://localhost/mydb", "test").unwrap();
/// let db = DatabaseConfig::from_config(&config).unwrap();
/// assert_eq!(db.pool_size, 10); // default applied
/// ```
///
/// Use `#[config(key = "...")]` to override the generated config key for a field.
/// This is useful when the YAML hierarchy doesn't match the Rust field name
/// (e.g., env var `OIDC_JWKS_URL` maps to `oidc.jwks.url`, not `oidc.jwks_url`):
///
/// ```ignore
/// #[derive(ConfigProperties, Clone, Debug)]
/// #[config(prefix = "oidc")]
/// pub struct OidcConfig {
///     pub issuer: Option<String>,
///     #[config(key = "jwks.url")]
///     pub jwks_url: Option<String>,          // reads from "oidc.jwks.url"
///     #[config(key = "client.id", default = "my-app")]
///     pub client_id: String,                  // reads from "oidc.client.id", defaults to "my-app"
/// }
/// ```
///
/// See `r2e-core/tests/config.rs` for runnable examples.
pub trait ConfigProperties: Sized {
    /// The configuration key prefix (e.g., `"app.database"`).
    fn prefix() -> &'static str;

    /// Metadata about all expected properties.
    fn properties_metadata() -> Vec<PropertyMeta>;

    /// Construct from an `R2eConfig` instance.
    fn from_config(config: &R2eConfig) -> Result<Self, ConfigError>;
}
