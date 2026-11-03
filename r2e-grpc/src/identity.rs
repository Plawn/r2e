//! Identity helpers for gRPC calls.
//!
//! - **Declarative** — an `#[inject(identity)]` parameter or struct field on
//!   a `#[grpc_routes]` service. It follows the HTTP identity contract to
//!   the letter: the call is seen as request [`Parts`](r2e_core::http::Parts)
//!   (see [`head`](crate::head)), and the identity type is extracted through
//!   [`FromRequestPartsVia`](r2e_core::web::extract::FromRequestPartsVia)
//!   against the application state — `AuthenticatedUser` reads the
//!   `authorization` metadata (the same header) with the
//!   `Arc<JwtClaimsValidator>` bean, compile-checked at
//!   `register_grpc_service()`. There is no gRPC-specific identity trait.
//! - **Manual** — [`extract_jwt_claims_from_metadata`] /
//!   [`GrpcIdentityExtractor`] over any [`JwtClaimsValidatorLike`], for
//!   handlers that want the raw [`StandardClaims`] from a [`MetadataMap`].

use std::sync::Arc;

use r2e_core::{Rejection, StandardClaims};
use tonic::metadata::MetadataMap;
use tonic::Status;

use crate::status::rejection_to_status;

/// **Removed.** gRPC identity goes through the HTTP contract now.
///
/// An `#[inject(identity)]` type on a `#[grpc_routes]` service is extracted
/// through [`FromRequestPartsVia`](r2e_core::web::extract::FromRequestPartsVia)
/// (or axum's `FromRequestParts`) from the call's request head, with the
/// application state — the validator is a bean. This empty trait is kept for
/// one release so an old `impl GrpcIdentity for …` fails to build pointing
/// at the migration guide, `docs/migration/grpc-identity.md`.
#[deprecated(
    since = "0.10.0",
    note = "`GrpcIdentity` was removed: implement `FromRequestPartsVia` (or `FromRequestParts`) \
            for the identity type instead — the validator becomes a bean. \
            See docs/migration/grpc-identity.md"
)]
pub trait GrpcIdentity {}

/// Extract and validate a JWT from gRPC metadata.
///
/// Looks for the `authorization` metadata key with a `Bearer ` prefix,
/// then validates the token using the provided `JwtClaimsValidator`.
///
/// Returns the validated [`StandardClaims`], or a `Status::unauthenticated` error.
pub async fn extract_jwt_claims_from_metadata<V: JwtClaimsValidatorLike>(
    metadata: &MetadataMap,
    validator: &V,
) -> Result<StandardClaims, Status> {
    let token = extract_bearer_token(metadata)?;
    validator
        .validate(token)
        .await
        .map_err(|e| Status::unauthenticated(format!("JWT validation failed: {e}")))
}

/// Extract the bearer token string from gRPC metadata as a typed
/// [`Rejection`] (`Unauthenticated` kind) — the form a hand-written
/// extractor over a [`MetadataMap`] uses.
///
/// Returns the token without the `Bearer ` prefix.
pub fn bearer_token(metadata: &MetadataMap) -> Result<&str, Rejection> {
    let auth_header = metadata
        .get("authorization")
        .ok_or_else(|| Rejection::new(r2e_core::RejectionKind::Unauthenticated, "Missing authorization metadata"))?;

    let auth_str = auth_header.to_str().map_err(|_| {
        Rejection::new(
            r2e_core::RejectionKind::Unauthenticated,
            "Invalid authorization metadata encoding",
        )
    })?;

    auth_str
        .strip_prefix("Bearer ")
        .or_else(|| auth_str.strip_prefix("bearer "))
        .ok_or_else(|| {
            Rejection::new(
                r2e_core::RejectionKind::Unauthenticated,
                "Authorization must use Bearer scheme",
            )
        })
}

/// Extract the bearer token string from gRPC metadata.
///
/// Returns the token without the `Bearer ` prefix, or a
/// `Status::unauthenticated` ([`bearer_token`] projected by
/// [`rejection_to_status`]).
pub fn extract_bearer_token(metadata: &MetadataMap) -> Result<&str, Status> {
    bearer_token(metadata).map_err(rejection_to_status)
}

/// Trait abstracting JWT claims validation.
///
/// This allows the gRPC identity extraction to work with any validator
/// that can validate tokens and return [`StandardClaims`]. The primary
/// implementation is `r2e_security::JwtClaimsValidator` (behind
/// `r2e-security`'s `grpc` feature — on by default through `r2e`'s `grpc` +
/// `security` features); the trait also allows testing with mock validators.
pub trait JwtClaimsValidatorLike: Send + Sync {
    fn validate(
        &self,
        token: &str,
    ) -> impl std::future::Future<
        Output = Result<StandardClaims, Box<dyn std::error::Error + Send + Sync>>,
    > + Send;
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
    ) -> Result<StandardClaims, Status> {
        extract_jwt_claims_from_metadata(metadata, validator).await
    }
}

/// Blanket implementation for `Arc<T>` where `T` implements `JwtClaimsValidatorLike`.
impl<T: JwtClaimsValidatorLike> JwtClaimsValidatorLike for Arc<T> {
    fn validate(
        &self,
        token: &str,
    ) -> impl std::future::Future<
        Output = Result<StandardClaims, Box<dyn std::error::Error + Send + Sync>>,
    > + Send {
        (**self).validate(token)
    }
}
