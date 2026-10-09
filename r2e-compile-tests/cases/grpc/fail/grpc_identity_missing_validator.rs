//! An `#[inject(identity)] AuthenticatedUser` parameter on a `#[grpc_routes]`
//! method needs the `Arc<JwtClaimsValidator>` bean (its `GrpcIdentity::Spec`
//! dep) — folded into `EndpointDeps`, so registering the service without
//! providing it is rejected at `register_grpc_service()`.

use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;
use r2e::r2e_security::AuthenticatedUser;

use r2e_compile_tests::proto::ping;

#[controller]
pub struct PingService {}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status>
    {
        let _ = (request, user);
        unimplemented!()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_grpc_service::<PingService>()
    };
}
