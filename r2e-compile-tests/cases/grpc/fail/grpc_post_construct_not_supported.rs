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
    // Async `&self`: without the disallow check this would land in the tonic
    // trait impl and fail with a confusing E0407 "not a member of trait".
    #[post_construct]
    async fn warm_up(&self) {}

    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

fn main() {}
