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
}

impl SecurityConfig {
    /// Create a new SecurityConfig with the given parameters and default cache TTL of 3600s.
    pub fn new(jwks_url: impl Into<String>, issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            jwks_url: jwks_url.into(),
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_cache_ttl_secs: 3600,
        }
    }

    /// Set the JWKS cache TTL in seconds.
    pub fn with_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.jwks_cache_ttl_secs = ttl_secs;
        self
    }
}
