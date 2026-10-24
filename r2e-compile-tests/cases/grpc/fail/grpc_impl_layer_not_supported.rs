use r2e::prelude::*;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs. (allow: the macro rejects the
// impl before emitting the code that would use the import.)
#[allow(unused_imports)]
use r2e_compile_tests::proto::ping;

#[controller]
pub struct TestGrpc;

// Tower layers are an HTTP-pipeline concept: a gRPC service has no
// per-method tower stack, at method or impl level.
#[grpc_routes(ping::ping_server::Ping)]
#[layer(r2e::http::DefaultBodyLimit::max(1024))]
impl TestGrpc {
    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

fn main() {}
