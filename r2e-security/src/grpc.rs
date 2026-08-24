//! gRPC bridge: use [`JwtClaimsValidator`] as the validator behind
//! `r2e_grpc::GrpcIdentityExtractor`.
//!
//! Feature `grpc`. With it, the `Arc<JwtClaimsValidator>` bean that
//! authenticates HTTP requests also authenticates gRPC calls:
//!
//! ```ignore
//! let claims = GrpcIdentityExtractor::extract_claims(request.metadata(), &self.jwt_validator).await?;
//! ```

use r2e_core::StandardClaims;
use r2e_grpc::identity::JwtClaimsValidatorLike;

use crate::jwt::JwtClaimsValidator;

impl JwtClaimsValidatorLike for JwtClaimsValidator {
    fn validate(
        &self,
        token: &str,
    ) -> impl std::future::Future<
        Output = Result<StandardClaims, Box<dyn std::error::Error + Send + Sync>>,
    > + Send {
        async move {
            JwtClaimsValidator::validate(self, token)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        }
    }
}
