//! Configuration for OpenFGA client.

use crate::error::OpenFgaError;
use serde::Deserialize;

fn default_connect_timeout() -> u64 { 10 }
fn default_request_timeout() -> u64 { 5 }
fn default_cache_enabled() -> bool { true }
fn default_cache_ttl() -> u64 { 60 }

/// Configuration for connecting to an OpenFGA server.
///
/// Can be deserialized from `application.yaml` via the R2E config system.
/// `endpoint` and `store_id` are required; all other fields have defaults.
///
/// ```yaml
/// openfga:
///   endpoint: "http://localhost:8080"
///   store_id: "my-store-id"
///   model_id: "model-id"        # optional
///   api_token: "secret"         # optional
///   cache_enabled: true         # default: true
///   cache_ttl_secs: 60          # default: 60
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct OpenFgaConfig {
    /// The OpenFGA server endpoint (e.g., "http://localhost:8080").
    pub endpoint: String,
    /// The store ID to use for authorization checks.
    pub store_id: String,
    /// Optional authorization model ID. If not set, uses the latest model.
    pub model_id: Option<String>,
    /// Optional API token for authentication.
    pub api_token: Option<String>,
    /// Connection timeout in seconds. Default: 10.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// Request timeout in seconds. Default: 5.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Whether to enable decision caching. Default: true.
    #[serde(default = "default_cache_enabled")]
    pub cache_enabled: bool,
    /// Cache TTL in seconds. Default: 60.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
}

impl OpenFgaConfig {
    /// Create a new OpenFGA configuration with the given endpoint and store ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use r2e_openfga::OpenFgaConfig;
    ///
    /// let config = OpenFgaConfig::new("http://localhost:8080", "store-id");
    /// ```
    pub fn new(endpoint: impl Into<String>, store_id: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            store_id: store_id.into(),
            model_id: None,
            api_token: None,
            connect_timeout_secs: 10,
            request_timeout_secs: 5,
            cache_enabled: true,
            cache_ttl_secs: 60,
        }
    }

    /// Set the authorization model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set the API token for authentication.
    pub fn with_api_token(mut self, token: impl Into<String>) -> Self {
        self.api_token = Some(token.into());
        self
    }

    /// Set the connection timeout in seconds.
    pub fn with_connect_timeout(mut self, secs: u64) -> Self {
        self.connect_timeout_secs = secs;
        self
    }

    /// Set the request timeout in seconds.
    pub fn with_request_timeout(mut self, secs: u64) -> Self {
        self.request_timeout_secs = secs;
        self
    }

    /// Enable or disable decision caching with the given TTL.
    pub fn with_cache(mut self, enabled: bool, ttl_secs: u64) -> Self {
        self.cache_enabled = enabled;
        self.cache_ttl_secs = ttl_secs;
        self
    }

    /// Disable decision caching.
    pub fn without_cache(mut self) -> Self {
        self.cache_enabled = false;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), OpenFgaError> {
        if self.endpoint.is_empty() {
            return Err(OpenFgaError::InvalidConfig("endpoint cannot be empty".into()));
        }
        if self.store_id.is_empty() {
            return Err(OpenFgaError::InvalidConfig("store_id cannot be empty".into()));
        }
        Ok(())
    }
}
