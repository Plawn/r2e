use jsonwebtoken::Algorithm;

/// Security configuration for JWT validation and JWKS cache.
#[derive(Clone, Debug)]
pub struct SecurityConfig {
    /// URL of the JWKS endpoint (e.g., https://auth.example.com/.well-known/jwks.json)
    pub jwks_url: String,

    /// Expected issuer in the "iss" claim
    pub issuer: String,

    /// Expected audience in the "aud" claim
    pub audience: String,

    /// JWKS cache TTL in seconds (default: 3600)
    pub jwks_cache_ttl_secs: u64,

    /// Minimum interval between JWKS refresh attempts in seconds (default: 10)
    pub jwks_min_refresh_interval_secs: u64,

    /// Allowed JWT algorithms. Tokens using other algorithms are rejected.
    /// Default: RS256 only.
    pub allowed_algorithms: Vec<Algorithm>,
}

impl SecurityConfig {
    /// Create a new SecurityConfig with the given parameters and default cache TTL of 3600s.
    pub fn new(jwks_url: impl Into<String>, issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            jwks_url: jwks_url.into(),
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_cache_ttl_secs: 3600,
            jwks_min_refresh_interval_secs: 10,
            allowed_algorithms: vec![Algorithm::RS256],
        }
    }

    /// Set the JWKS cache TTL in seconds.
    pub fn with_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.jwks_cache_ttl_secs = ttl_secs;
        self
    }

    /// Set the minimum interval between JWKS refresh attempts.
    pub fn with_min_refresh_interval(mut self, interval_secs: u64) -> Self {
        self.jwks_min_refresh_interval_secs = interval_secs;
        self
    }

    /// Set the allowed JWT algorithms. Empty lists will cause validation to fail.
    pub fn with_allowed_algorithms(
        mut self,
        algorithms: impl IntoIterator<Item = Algorithm>,
    ) -> Self {
        self.allowed_algorithms = algorithms.into_iter().collect();
        self
    }

    /// Convenience method to allow a single algorithm.
    pub fn with_allowed_algorithm(mut self, algorithm: Algorithm) -> Self {
        self.allowed_algorithms = vec![algorithm];
        self
    }
}
