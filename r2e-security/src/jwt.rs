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
    Static(Arc<DecodingKey>),
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
    validation: Validation,
}

impl JwtClaimsValidator {
    fn build_validation(config: &SecurityConfig) -> Validation {
        // `algorithms` is replaced below, so this fallback is only needed to
        // construct a Validation when the configured allow-list is empty.
        let mut validation = Validation::new(
            config
                .allowed_algorithms
                .first()
                .copied()
                .unwrap_or(jsonwebtoken::Algorithm::RS256),
        );
        validation.algorithms = config.allowed_algorithms.clone();
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&[&config.audience]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation
    }

    /// Create a new validator backed by a JWKS cache.
    pub fn new(jwks: Arc<JwksCache>, config: SecurityConfig) -> Self {
        let validation = Self::build_validation(&config);
        Self {
            key_source: KeySource::Jwks(jwks),
            config,
            validation,
        }
    }

    /// Create a new validator with a static decoding key (useful for testing).
    pub fn new_with_static_key(key: DecodingKey, config: SecurityConfig) -> Self {
        let validation = Self::build_validation(&config);
        Self {
            key_source: KeySource::Static(Arc::new(key)),
            config,
            validation,
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
    /// 5. Subject presence: the token is rejected if it has no non-empty `sub` claim
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
            KeySource::Static(key) => Arc::clone(key),
            KeySource::Jwks(jwks) => {
                let kid = header.kid.as_deref().ok_or_else(|| {
                    SecurityError::InvalidToken("JWT header missing 'kid' field".into())
                })?;
                jwks.get_shared_key(kid, algorithm).await?
            }
        };

        // Step 3: Decode and validate the token using the parameters prepared
        // once when the validator was constructed.
        let token_data = decode::<serde_json::Value>(token, &decoding_key, &self.validation)
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

        // A token must identify its subject. Without a non-empty `sub`, any
        // authorization keyed on the user identity would operate on an empty or
        // ambiguous identifier, so reject it outright.
        let sub = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                warn!("JWT rejected: missing or empty 'sub' (subject) claim");
                SecurityError::ValidationFailed("Token has no 'sub' (subject) claim".into())
            })?;

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
    pub fn with_role_extractor<R: RoleExtractor>(
        self,
        extractor: R,
    ) -> JwtValidator<IdentityBuilderWith<R>> {
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
