use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use tracing::{debug, warn};

use crate::config::SecurityConfig;
use crate::error::SecurityError;
use crate::identity::{DefaultIdentityBuilder, IdentityBuilder, RoleExtractor};
use crate::jwks::JwksCache;

/// Source of decoding keys: either a JWKS cache or a static key for testing.
enum KeySource {
    Jwks(Arc<JwksCache>),
    Static(DecodingKey),
}

/// JWT token validator.
///
/// Validates JWT tokens by checking the signature (via JWKS or a static key),
/// and verifying the issuer, audience, and expiration claims.
///
/// The type parameter `B` controls how validated JWT claims are mapped to an
/// identity type. The default ([`DefaultIdentityBuilder`]) produces
/// [`AuthenticatedUser`](crate::AuthenticatedUser).
///
/// # Custom identity type
///
/// ```ignore
/// let validator = JwtValidator::from_jwks(jwks, config, MyIdentityBuilder);
/// // validator.validate(token) returns Result<MyUser, SecurityError>
/// ```
pub struct JwtValidator<B: IdentityBuilder = DefaultIdentityBuilder> {
    key_source: KeySource,
    config: SecurityConfig,
    identity_builder: B,
}

/// Convenience constructors for the default identity type ([`AuthenticatedUser`](crate::AuthenticatedUser)).
impl JwtValidator {
    /// Create a new JwtValidator backed by a JWKS cache.
    pub fn new(jwks: Arc<JwksCache>, config: SecurityConfig) -> Self {
        Self::from_jwks(jwks, config, DefaultIdentityBuilder::new())
    }

    /// Create a new JwtValidator with a static decoding key (useful for testing).
    ///
    /// This bypasses the JWKS cache entirely and uses the provided key directly.
    pub fn new_with_static_key(key: DecodingKey, config: SecurityConfig) -> Self {
        Self::from_static_key(key, config, DefaultIdentityBuilder::new())
    }

    /// Set a custom role extractor.
    pub fn with_role_extractor(mut self, extractor: Box<dyn RoleExtractor>) -> Self {
        self.identity_builder = DefaultIdentityBuilder::with_extractor(extractor);
        self
    }
}

/// Generic constructors and validation for any identity builder.
impl<B: IdentityBuilder> JwtValidator<B> {
    /// Create a JwtValidator backed by a JWKS cache with a custom identity builder.
    pub fn from_jwks(jwks: Arc<JwksCache>, config: SecurityConfig, identity_builder: B) -> Self {
        Self {
            key_source: KeySource::Jwks(jwks),
            config,
            identity_builder,
        }
    }

    /// Create a JwtValidator with a static decoding key and a custom identity builder.
    pub fn from_static_key(
        key: DecodingKey,
        config: SecurityConfig,
        identity_builder: B,
    ) -> Self {
        Self {
            key_source: KeySource::Static(key),
            config,
            identity_builder,
        }
    }

    /// Validate a JWT token and return the identity on success.
    ///
    /// Steps:
    /// 1. Decode the JWT header to extract the `kid`
    /// 2. Retrieve the decoding key (from JWKS cache or static key)
    /// 3. Validate signature + standard claims (iss, aud, exp)
    /// 4. Build the identity via the [`IdentityBuilder`]
    pub async fn validate(&self, token: &str) -> Result<B::Identity, SecurityError> {
        // Step 1: Decode header to get kid and algorithm
        let header = decode_header(token)
            .map_err(|e| SecurityError::InvalidToken(format!("Failed to decode header: {e}")))?;

        let algorithm = header.alg;
        debug!(?algorithm, kid = ?header.kid, "Decoded JWT header");

        // Step 2: Get the decoding key
        let decoding_key = match &self.key_source {
            KeySource::Static(key) => key.clone(),
            KeySource::Jwks(jwks) => {
                let kid = header.kid.as_deref().ok_or_else(|| {
                    SecurityError::InvalidToken("JWT header missing 'kid' field".into())
                })?;
                jwks.get_key(kid).await?
            }
        };

        // Step 3: Set up validation parameters
        let mut validation = Validation::new(algorithm);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        // Step 4: Decode and validate the token
        let token_data = decode::<serde_json::Value>(token, &decoding_key, &validation)
            .map_err(|e| {
                let err = match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                        SecurityError::TokenExpired
                    }
                    jsonwebtoken::errors::ErrorKind::InvalidIssuer => {
                        SecurityError::ValidationFailed("Invalid issuer".into())
                    }
                    jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                        SecurityError::ValidationFailed("Invalid audience".into())
                    }
                    _ => SecurityError::InvalidToken(e.to_string()),
                };
                warn!(error = %err, "JWT claim validation failed");
                err
            })?;

        // Step 5: Build identity from validated claims
        let sub = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        let identity = self.identity_builder.build(token_data.claims).await?;

        debug!(sub = %sub, "JWT validated");
        Ok(identity)
    }
}
