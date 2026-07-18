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
    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }

    // Sync helper: without the disallow check this would be re-emitted
    // verbatim into `other_methods` and silently never run.
    #[pre_destroy]
    fn close(&self) {}
}

fn main() {}
