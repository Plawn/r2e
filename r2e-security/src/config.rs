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

    /// Total timeout for a JWKS HTTP request in seconds (default: 10).
    /// Bounds how long request authentication can block on a slow JWKS endpoint.
    pub jwks_request_timeout_secs: u64,

    /// TCP connect timeout for a JWKS HTTP request in seconds (default: 5).
    pub jwks_connect_timeout_secs: u64,

    /// Maximum accepted size of a JWKS HTTP response body in bytes (default: 1 MiB).
    /// Protects against a compromised/hostile endpoint returning an unbounded body.
    pub jwks_max_response_bytes: u64,

    /// Allow fetching the JWKS over a non-HTTPS URL. Default: `false`.
    /// Enable only for local development — an `http://` JWKS URL lets a network
    /// MITM substitute signing keys and forge tokens.
    pub allow_insecure_jwks_url: bool,

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
            jwks_request_timeout_secs: 10,
            jwks_connect_timeout_secs: 5,
            jwks_max_response_bytes: 1024 * 1024,
            allow_insecure_jwks_url: false,
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

    /// Set the total timeout for JWKS HTTP requests.
    pub fn with_request_timeout(mut self, timeout_secs: u64) -> Self {
        self.jwks_request_timeout_secs = timeout_secs;
        self
    }

    /// Set the TCP connect timeout for JWKS HTTP requests.
    pub fn with_connect_timeout(mut self, timeout_secs: u64) -> Self {
        self.jwks_connect_timeout_secs = timeout_secs;
        self
    }

    /// Set the maximum accepted JWKS response body size in bytes.
    pub fn with_max_response_bytes(mut self, max_bytes: u64) -> Self {
        self.jwks_max_response_bytes = max_bytes;
        self
    }

    /// Allow fetching the JWKS over a non-HTTPS URL (local development only).
    pub fn allow_insecure_jwks_url(mut self) -> Self {
        self.allow_insecure_jwks_url = true;
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
