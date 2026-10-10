//! gRPC bridge (feature `grpc`): the `Arc<JwtClaimsValidator>` bean that
//! authenticates HTTP requests also authenticates gRPC calls.
//!
//! - [`AuthenticatedUser`] implements [`GrpcIdentity`], so an
//!   `#[inject(identity)] user: AuthenticatedUser` parameter on a
//!   `#[grpc_routes]` method is extracted from the `authorization` metadata
//!   (Bearer scheme) and validated with the `Arc<JwtClaimsValidator>` bean
//!   — read once at registration through [`JwtIdentitySpec`] and
//!   compile-checked at `register_grpc_service()` like any decorator dep.
//! - [`JwtClaimsValidator`] implements `r2e_grpc::JwtClaimsValidatorLike`
//!   for the manual path (`GrpcIdentityExtractor::extract_claims`).
//!
//! ```ignore
//! #[grpc_routes(proto::greeter::greeter_server::Greeter)]
//! impl GreeterService {
//!     #[roles("admin")]
//!     async fn say_hello_admin(
//!         &self,
//!         request: tonic::Request<HelloRequest>,
//!         #[inject(identity)] user: AuthenticatedUser,
//!     ) -> Result<tonic::Response<HelloReply>, tonic::Status> { .. }
//! }
//! ```

use std::sync::Arc;

use r2e_core::beans::BeanContext;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{DecoratorSpec, Rejection, StandardClaims};
use r2e_grpc::identity::{bearer_token, GrpcIdentity, JwtClaimsValidatorLike};
use r2e_grpc::tonic::metadata::MetadataMap;

use crate::identity::AuthenticatedUser;
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

/// The [`DecoratorSpec`] behind [`AuthenticatedUser`]'s gRPC identity
/// extraction: reads the `Arc<JwtClaimsValidator>` bean from the graph.
#[derive(Debug, Clone, Copy, Default)]
pub struct JwtIdentitySpec;

impl DecoratorSpec for JwtIdentitySpec {
    type Product = Arc<JwtClaimsValidator>;
    type Deps = TCons<Arc<JwtClaimsValidator>, TNil>;

    fn build(self, ctx: &BeanContext) -> Self::Product {
        ctx.get::<Arc<JwtClaimsValidator>>()
    }
}

impl GrpcIdentity for AuthenticatedUser {
    type Spec = JwtIdentitySpec;

    fn spec() -> Self::Spec {
        JwtIdentitySpec
    }

    fn extract(
        validator: &Arc<JwtClaimsValidator>,
        metadata: &MetadataMap,
    ) -> impl std::future::Future<Output = Result<Self, Rejection>> + Send {
        async move {
            let token = bearer_token(metadata)?;
            let claims = JwtClaimsValidator::validate(validator, token)
                .await
                .map_err(|e| {
                    tracing::warn!(error = %e, "gRPC JWT validation failed");
                    Rejection::from(e)
                })?;
            Ok(AuthenticatedUser::from_claims(claims))
        }
    }
}
