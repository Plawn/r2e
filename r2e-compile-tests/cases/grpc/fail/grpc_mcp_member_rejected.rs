//! MCP members belong on a `#[routes]` impl: `#[grpc_routes]` only accepts
//! gRPC members, and says where the MCP member goes.

use r2e::prelude::*;

#[allow(unused_imports)]
use r2e_compile_tests::proto::ping;

#[controller]
pub struct TestGrpc;

#[grpc_routes(ping::ping_server::Ping)]
impl TestGrpc {
    #[tool]
    async fn echo(&self) -> String {
        "echo".to_string()
    }

    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

fn main() {}
