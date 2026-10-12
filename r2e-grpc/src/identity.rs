//! Identity extraction for gRPC calls.
//!
//! Two paths exist:
//!
//! - **Declarative** — an `#[inject(identity)]` parameter on a
//!   `#[grpc_routes]` method. The generated dispatch builds the type's
//!   [`GrpcIdentity::Spec`] once at registration (from the bean graph, like
//!   any decorator spec) and runs [`GrpcIdentity::extract`] per call, before
//!   guards; a failure is projected onto a [`tonic::Status`] by kind.
//!   `r2e-security` implements it for `AuthenticatedUser` under its `grpc`
//!   feature.
//! - **Manual** — [`extract_jwt_claims_from_metadata`] /
//!   [`GrpcIdentityExtractor`] over any [`JwtClaimsValidatorLike`], for
//!   handlers that want the raw [`StandardClaims`].

use std::future::Future;
use std::sync::Arc;

use r2e_core::{DecoratorSpec, Identity, Rejection, StandardClaims};
use tonic::metadata::MetadataMap;
use tonic::Status;

use crate::status::rejection_to_status;

/// An identity type that `#[grpc_routes]` can inject from request metadata.
///
/// The extractor (the validator bean, typically) is built **once at
/// registration** from the bean graph through [`Self::Spec`] — exactly like a
/// guard or interceptor spec — and its [`DecoratorSpec::Deps`] are folded into
/// the service's [`EndpointDeps`](r2e_core::EndpointDeps), so a missing
/// validator bean is a compile error at `register_grpc_service()`.
pub trait GrpcIdentity: Identity + Sized {
    /// The spec that builds the per-service extractor from the bean context.
    type Spec: DecoratorSpec;

    /// The spec value to build the extractor with.
    fn spec() -> Self::Spec;

    /// Extract a **required** identity from the call metadata.
    fn extract(
        extractor: &<Self::Spec as DecoratorSpec>::Product,
        metadata: &MetadataMap,
    ) -> impl Future<Output = Result<Self, Rejection>> + Send;

    /// Extract an **optional** identity (`Option<Self>` parameters): `None`
    /// when no credential is presented, `Err` when one is presented but
    /// rejected — a bad token on an optional identity is still a failure,
    /// never silently anonymous. Default: no `authorization` metadata ⇒
    /// `None`.
    fn extract_optional(
        extractor: &<Self::Spec as DecoratorSpec>::Product,
        metadata: &MetadataMap,
    ) -> impl Future<Output = Result<Option<Self>, Rejection>> + Send {
        async move {
            if metadata.get("authorization").is_none() {
                return Ok(None);
            }
            Self::extract(extractor, metadata).await.map(Some)
        }
    }
}

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
/// [`Rejection`] (`Unauthenticated` kind) — the form [`GrpcIdentity`]
/// implementations use.
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
