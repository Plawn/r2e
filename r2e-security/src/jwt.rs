use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use tracing::{debug, warn};

use crate::config::SecurityConfig;
use crate::error::SecurityError;
use crate::identity::{DefaultIdentityBuilder, IdentityBuilder, IdentityBuilderWith};
use crate::jwks::JwksCache;
use crate::openid::RoleExtractor;

/// Source of decoding keys: either a JWKS cache or a static key for testing.
enum KeySource {
    Jwks(Arc<JwksCache>),
    Static(DecodingKey),
}

/// Low-level JWT claims validator.
///
/// Validates JWT tokens and returns raw claims without building an identity.
/// Use this when you need to implement custom identity extraction logic,
/// or when you want multiple identity types backed by the same validator.
///
/// For most use cases, prefer [`JwtValidator`] which combines validation
/// with identity building.
///
/// # Example
///
/// ```ignore
/// // Validate and get raw claims
/// let claims = claims_validator.validate(token).await?;
/// let sub = claims["sub"].as_str().unwrap();
///
/// // Build different identity types from the same claims
/// let light_user = AuthenticatedUser::from_claims(claims.clone());
/// let full_user = db_lookup(sub, &pool).await?;
/// ```
pub struct JwtClaimsValidator {
    key_source: KeySource,
    config: SecurityConfig,
}

impl JwtClaimsValidator {
    /// Create a new validator backed by a JWKS cache.
    pub fn new(jwks: Arc<JwksCache>, config: SecurityConfig) -> Self {
        Self {
            key_source: KeySource::Jwks(jwks),
            config,
        }
    }

    /// Create a new validator with a static decoding key (useful for testing).
    pub fn new_with_static_key(key: DecodingKey, config: SecurityConfig) -> Self {
        Self {
            key_source: KeySource::Static(key),
            config,
        }
    }

    /// Returns the security configuration.
    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    /// Validate a JWT token and return the raw claims.
    ///
    /// This performs:
    /// 1. Header decoding to extract `kid` and algorithm
    /// 2. Key retrieval (from JWKS cache or static key)
    /// 3. Signature validation
    /// 4. Standard claims validation (iss, aud, exp, nbf)
    ///
    /// Returns the validated claims as a JSON value.
    pub async fn validate(&self, token: &str) -> Result<serde_json::Value, SecurityError> {
        // Step 1: Decode header to get kid and algorithm
        let header = decode_header(token)
            .map_err(|e| SecurityError::InvalidToken(format!("Failed to decode header: {e}")))?;

        let algorithm = header.alg;
        debug!(?algorithm, kid = ?header.kid, "Decoded JWT header");

        if self.config.allowed_algorithms.is_empty() {
            return Err(SecurityError::ValidationFailed(
                "No allowed JWT algorithms configured".into(),
            ));
        }

        if !self.config.allowed_algorithms.contains(&algorithm) {
            return Err(SecurityError::ValidationFailed(format!(
                "Disallowed JWT algorithm: {algorithm:?}"
            )));
        }

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
        validation.algorithms = self.config.allowed_algorithms.clone();
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

        let sub = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        debug!(sub = %sub, "JWT validated");
        Ok(token_data.claims)
    }

    /// Create a full [`JwtValidator`] by combining this claims validator
    /// with an identity builder.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let claims_validator = JwtClaimsValidator::new(jwks, config);
    ///
    /// // Create validators for different identity types
    /// let light_validator = claims_validator.with_identity_builder(DefaultIdentityBuilder::default());
    /// let custom_validator = claims_validator.with_identity_builder(MyIdentityBuilder::new(pool));
    /// ```
    pub fn with_identity_builder<B: IdentityBuilder>(self, builder: B) -> JwtValidator<B> {
        JwtValidator {
            claims_validator: Arc::new(self),
            identity_builder: builder,
        }
    }

    /// Create a full [`JwtValidator`] with a custom role extractor.
    ///
    /// This is a convenience method that creates an [`IdentityBuilderWith`]
    /// using the provided role extractor.
    pub fn with_role_extractor<R: RoleExtractor>(self, extractor: R) -> JwtValidator<IdentityBuilderWith<R>> {
        self.with_identity_builder(IdentityBuilderWith::new(extractor))
    }
}

/// JWT token validator with identity building.
///
/// Validates JWT tokens and builds an identity using the configured
/// [`IdentityBuilder`]. This is the main entry point for JWT authentication.
///
/// # Multiple identity types
///
/// If you need different identity types for different endpoints (e.g., a light
/// `AuthenticatedUser` for most endpoints and a full `DbUser` with database
/// lookup for others), use [`JwtClaimsValidator`] directly and implement
/// custom `FromRequestParts` extractors.
///
/// # Example
///
/// ```ignore
/// // Default: produces AuthenticatedUser
/// let validator = JwtValidator::new(jwks, config);
///
/// // Custom identity builder
/// let validator = JwtValidator::from_jwks(jwks, config, MyIdentityBuilder::new());
/// ```
pub struct JwtValidator<B: IdentityBuilder = DefaultIdentityBuilder> {
    claims_validator: Arc<JwtClaimsValidator>,
    identity_builder: B,
}

impl JwtValidator {
    /// Create a new JwtValidator backed by a JWKS cache.
    ///
    /// Uses the default identity builder which produces [`AuthenticatedUser`](crate::AuthenticatedUser).
    pub fn new(jwks: Arc<JwksCache>, config: SecurityConfig) -> Self {
        JwtClaimsValidator::new(jwks, config)
            .with_identity_builder(DefaultIdentityBuilder::default())
    }

    /// Create a new JwtValidator with a static decoding key (useful for testing).
    pub fn new_with_static_key(key: DecodingKey, config: SecurityConfig) -> Self {
        JwtClaimsValidator::new_with_static_key(key, config)
            .with_identity_builder(DefaultIdentityBuilder::default())
    }
}

impl<B: IdentityBuilder> JwtValidator<B> {
    /// Create a JwtValidator backed by a JWKS cache with a custom identity builder.
    pub fn from_jwks(jwks: Arc<JwksCache>, config: SecurityConfig, identity_builder: B) -> Self {
        JwtClaimsValidator::new(jwks, config).with_identity_builder(identity_builder)
    }

    /// Create a JwtValidator with a static decoding key and a custom identity builder.
    pub fn from_static_key(key: DecodingKey, config: SecurityConfig, identity_builder: B) -> Self {
        JwtClaimsValidator::new_with_static_key(key, config).with_identity_builder(identity_builder)
    }

    /// Returns a reference to the underlying claims validator.
    ///
    /// Use this to share the same JWT validation logic across multiple
    /// identity types.
    pub fn claims_validator(&self) -> &Arc<JwtClaimsValidator> {
        &self.claims_validator
    }

    /// Returns the security configuration.
    pub fn config(&self) -> &SecurityConfig {
        self.claims_validator.config()
    }

    /// Validate a JWT token and return the identity on success.
    pub async fn validate(&self, token: &str) -> Result<B::Identity, SecurityError> {
        let claims = self.claims_validator.validate(token).await?;
        self.identity_builder.build(claims).await
    }

    /// Validate a JWT token and return raw claims without building an identity.
    ///
    /// This is useful when you need the claims for custom processing
    /// in addition to or instead of the built identity.
    pub async fn validate_claims(&self, token: &str) -> Result<serde_json::Value, SecurityError> {
        self.claims_validator.validate(token).await
    }
}
