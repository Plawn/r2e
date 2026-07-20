use url::Url;

use crate::error::OidcError;

/// Configuration for the embedded OAuth/JWT issuer.
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
    ///
    /// Leave empty to derive a stable key ID from the generated or loaded public key.
    pub kid: String,
    /// Whether the resource owner password credentials grant is enabled.
    ///
    /// This grant is disabled by default because OAuth 2.0 Security BCP forbids
    /// its use for production systems. Enable it only for development fixtures.
    pub password_grant_enabled: bool,
    /// Maximum concurrent Argon2 password/secret verifications.
    pub max_credential_verifications: usize,
}

impl Default for OidcServerConfig {
    fn default() -> Self {
        Self {
            issuer: "http://localhost:3000".into(),
            audience: "r2e-app".into(),
            token_ttl_secs: 3600,
            base_path: String::new(),
            kid: String::new(),
            password_grant_enabled: false,
            max_credential_verifications: 32,
        }
    }
}

impl OidcServerConfig {
    /// Return the canonical issuer actually used in JWT `iss` claims and metadata.
    ///
    /// `base_path` is part of the issuer namespace. For example, issuer
    /// `https://auth.example.com` plus base path `/auth` yields
    /// `https://auth.example.com/auth`.
    pub fn canonical_issuer(&self) -> String {
        format!(
            "{}{}",
            self.issuer.trim_end_matches('/'),
            normalize_base_path_for_url(&self.base_path)
        )
    }

    pub(crate) fn validate(&self) -> Result<(), OidcError> {
        let issuer = Url::parse(&self.issuer)
            .map_err(|e| OidcError::Configuration(format!("invalid issuer URL: {e}")))?;

        if issuer.scheme() != "https" && issuer.host_str() != Some("localhost") {
            return Err(OidcError::Configuration(
                "issuer must use https outside localhost development".into(),
            ));
        }

        if issuer.query().is_some() || issuer.fragment().is_some() {
            return Err(OidcError::Configuration(
                "issuer must not contain query or fragment components".into(),
            ));
        }

        if self.audience.trim().is_empty() {
            return Err(OidcError::Configuration(
                "audience must not be empty".into(),
            ));
        }

        if self.token_ttl_secs == 0 {
            return Err(OidcError::Configuration(
                "token TTL must be greater than zero".into(),
            ));
        }

        if self.max_credential_verifications == 0 {
            return Err(OidcError::Configuration(
                "max credential verifications must be greater than zero".into(),
            ));
        }

        validate_base_path(&self.base_path)?;

        if !self.kid.is_empty() && self.kid.trim().is_empty() {
            return Err(OidcError::Configuration(
                "kid must not be whitespace".into(),
            ));
        }

        Ok(())
    }
}

fn normalize_base_path_for_url(base_path: &str) -> String {
    if base_path.is_empty() || base_path == "/" {
        String::new()
    } else {
        format!("/{}", base_path.trim_matches('/'))
    }
}

fn validate_base_path(base_path: &str) -> Result<(), OidcError> {
    if base_path.is_empty() {
        return Ok(());
    }

    if !base_path.starts_with('/') {
        return Err(OidcError::Configuration(
            "base_path must be empty or start with '/'".into(),
        ));
    }

    if base_path.len() > 1 && base_path.ends_with('/') {
        return Err(OidcError::Configuration(
            "base_path must not end with '/'".into(),
        ));
    }

    if base_path.contains('?') || base_path.contains('#') {
        return Err(OidcError::Configuration(
            "base_path must not contain query or fragment components".into(),
        ));
    }

    Ok(())
}
