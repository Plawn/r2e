/// Configuration for the embedded OIDC server.
#[derive(Clone, Debug)]
pub struct OidcServerConfig {
    /// JWT issuer claim (`iss`).
    pub issuer: String,
    /// JWT audience claim (`aud`).
    pub audience: String,
    /// Token time-to-live in seconds.
    pub token_ttl_secs: u64,
    /// Base path for OIDC endpoints (e.g. `""` for root, `"/auth"` for `/auth/oauth/token`).
    pub base_path: String,
    /// Key ID (`kid`) included in JWT headers and JWKS.
    pub kid: String,
}

impl Default for OidcServerConfig {
    fn default() -> Self {
        Self {
            issuer: "http://localhost:3000".into(),
            audience: "r2e-app".into(),
            token_ttl_secs: 3600,
            base_path: String::new(),
            kid: "r2e-oidc-key-1".into(),
        }
    }
}
