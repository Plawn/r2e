//! gRPC bridge (feature `grpc`): the manual-path glue between the
//! `Arc<JwtClaimsValidator>` bean and `r2e-grpc`'s metadata helpers.
//!
//! A `#[grpc_routes]` service needs nothing from this module to authenticate:
//! r2e-grpc builds an HTTP head per call (the metadata becomes the headers),
//! so [`AuthenticatedUser`](crate::AuthenticatedUser) is extracted through the
//! very `FromRequestPartsVia` impl HTTP routes use — as a struct-level
//! `#[inject(identity)]` field or a method parameter — and the validator bean
//! is compile-checked at `register_grpc_service()`.
//!
//! What stays here is the manual path for hand-written tonic services:
//! [`JwtClaimsValidator`] implements `r2e_grpc::JwtClaimsValidatorLike`, so
//! `r2e_grpc::identity::extract_jwt_claims_from_metadata` /
//! `GrpcIdentityExtractor::extract_claims` validate a call's bearer token
//! against the bean.
//!
//! ```ignore
//! #[derive(Controller)]
//! #[controller]
//! pub struct GreeterService {
//!     #[inject(identity)]
//!     user: AuthenticatedUser,
//! }
//!
//! #[grpc_routes(proto::greeter::greeter_server::Greeter)]
//! impl GreeterService {
//!     #[roles("admin")]
//!     async fn say_hello_admin(
//!         &self,
//!         request: tonic::Request<HelloRequest>,
//!     ) -> Result<tonic::Response<HelloReply>, tonic::Status> { .. }
//! }
//! ```

use r2e_core::StandardClaims;
use r2e_grpc::identity::JwtClaimsValidatorLike;

use crate::jwt::JwtClaimsValidator;

impl JwtClaimsValidatorLike for JwtClaimsValidator {
    async fn validate(
        &self,
        token: &str,
    ) -> Result<StandardClaims, Box<dyn std::error::Error + Send + Sync>> {
        JwtClaimsValidator::validate(self, token)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}
