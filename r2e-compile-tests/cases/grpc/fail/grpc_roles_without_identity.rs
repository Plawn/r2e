//! `#[roles]` on a `#[grpc_routes]` method without an `#[inject(identity)]`
//! parameter: `RolesGuard` needs a `RoleBasedIdentity`, and the method can
//! only offer `NoIdentity` — rejected at compile time, never silently open.

use r2e::prelude::*;

use r2e_compile_tests::proto::ping;

#[controller]
pub struct PingService {}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    #[roles("admin")]
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status>
    {
        let _ = request;
        unimplemented!()
    }
}

fn main() {}
