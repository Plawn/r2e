use std::sync::Arc;

use tonic::metadata::MetadataMap;
use tonic::Status;

/// Extract and validate a JWT from gRPC metadata.
///
/// Looks for the `authorization` metadata key with a `Bearer ` prefix,
/// then validates the token using the provided `JwtClaimsValidator`.
///
/// Returns the validated claims as a JSON value, or a `Status::unauthenticated` error.
pub async fn extract_jwt_claims_from_metadata<V: JwtClaimsValidatorLike>(
    metadata: &MetadataMap,
    validator: &V,
) -> Result<serde_json::Value, Status> {
    let token = extract_bearer_token(metadata)?;
    validator
        .validate(token)
        .await
        .map_err(|e| Status::unauthenticated(format!("JWT validation failed: {e}")))
}

/// Extract the bearer token string from gRPC metadata.
///
/// Returns the token without the `Bearer ` prefix.
pub fn extract_bearer_token(metadata: &MetadataMap) -> Result<&str, Status> {
    let auth_header = metadata
        .get("authorization")
        .ok_or_else(|| Status::unauthenticated("Missing authorization metadata"))?;

    let auth_str = auth_header
        .to_str()
        .map_err(|_| Status::unauthenticated("Invalid authorization metadata encoding"))?;

    auth_str
        .strip_prefix("Bearer ")
        .or_else(|| auth_str.strip_prefix("bearer "))
        .ok_or_else(|| Status::unauthenticated("Authorization must use Bearer scheme"))
}

/// Trait abstracting JWT claims validation.
///
/// This allows the gRPC identity extraction to work with any validator
/// that can validate tokens and return claims. The primary implementation
/// is `r2e_security::JwtClaimsValidator`, but this trait allows testing
/// with mock validators.
pub trait JwtClaimsValidatorLike: Send + Sync {
    fn validate(
        &self,
        token: &str,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>>
           + Send;
}

/// Wrapper to use a gRPC identity extractor with any type that holds an
/// `Arc<JwtClaimsValidator>` in the app state.
///
/// This is used by generated code to extract identity from gRPC requests.
pub struct GrpcIdentityExtractor;

impl GrpcIdentityExtractor {
    /// Extract identity claims from gRPC metadata using a validator from the app state.
    ///
    /// The `validator` is typically obtained via `Arc<JwtClaimsValidator>::from_ref(state)`.
    pub async fn extract_claims<V: JwtClaimsValidatorLike>(
        metadata: &MetadataMap,
        validator: &V,
    ) -> Result<serde_json::Value, Status> {
        extract_jwt_claims_from_metadata(metadata, validator).await
    }
}

/// Blanket implementation for `Arc<T>` where `T` implements `JwtClaimsValidatorLike`.
impl<T: JwtClaimsValidatorLike> JwtClaimsValidatorLike for Arc<T> {
    fn validate(
        &self,
        token: &str,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>>
           + Send {
        (**self).validate(token)
    }
}
