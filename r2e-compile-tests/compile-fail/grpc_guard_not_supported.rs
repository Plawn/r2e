use r2e::prelude::*;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs. (allow: the macro rejects the
// impl before emitting the code that would use the import.)
#[allow(unused_imports)]
use r2e_compile_tests::proto::ping;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct TestGrpc;

#[grpc_routes(ping::ping_server::Ping)]
impl TestGrpc {
    #[guard(MyGuard)]
    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

pub struct MyGuard;

fn main() {}
