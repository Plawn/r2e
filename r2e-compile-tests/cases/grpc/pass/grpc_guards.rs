//! The full post-auth guard family compiles on `#[grpc_routes]`: impl-level
//! `#[guard]`, method-level `#[guard]`/`#[roles]`/`#[all_roles]` over a
//! required or optional `#[inject(identity)]` parameter, identity in any
//! parameter position, and `#[intercept]` alongside. The identity's
//! validator bean and the guard's bean are folded into `EndpointDeps`, so
//! `register_grpc_service()` type-checks only with both provided.

use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;
use r2e::r2e_security::jwt::JwtClaimsValidator;
use r2e::r2e_security::AuthenticatedUser;
use std::future::Future;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiKeys;

#[derive(DecoratorBean)]
pub struct ApiKey {
    #[inject]
    keys: ApiKeys,
}

impl<I: Identity> Guard<I> for ApiKey {
    fn check(&self, _ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Rejection>> + Send {
        let _ = &self.keys;
        async move { Ok(()) }
    }
}

use r2e_compile_tests::proto::ping;

type Req = r2e::r2e_grpc::tonic::Request<ping::PingRequest>;
type Resp = Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status>;

#[controller]
pub struct PingService {}

#[grpc_routes(ping::ping_server::Ping)]
#[guard(ApiKey::spec())]
impl PingService {
    #[roles("admin")]
    #[intercept(Logged::info())]
    async fn ping(&self, #[inject(identity)] user: AuthenticatedUser, request: Req) -> Resp {
        let _ = (user, request);
        unimplemented!()
    }
}

#[controller]
pub struct OptionalPingService {}

#[grpc_routes(ping::ping_server::Ping)]
impl OptionalPingService {
    #[guard(ApiKey::spec())]
    async fn ping(&self, request: Req, #[inject(identity)] user: Option<AuthenticatedUser>) -> Resp {
        let _ = (user, request);
        unimplemented!()
    }
}

#[controller]
pub struct AnonymousPingService {}

#[grpc_routes(ping::ping_server::Ping)]
impl AnonymousPingService {
    #[guard(ApiKey::spec())]
    async fn ping(&self, request: Req) -> Resp {
        let _ = request;
        unimplemented!()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .provide(ApiKeys)
            .provide(Arc::new(r2e_test::TestJwt::new().claims_validator()) as Arc<JwtClaimsValidator>)
            .build_state()
            .await
            .register_grpc_service::<PingService>()
            .register_grpc_service::<OptionalPingService>()
            .register_grpc_service::<AnonymousPingService>()
    };
}
