//! Listing the same gRPC service twice in one `grpc_services(..)` tuple is a
//! macro error at the `#[module]` attribute — the cheap compile-time half of
//! the one-owner rule (two *different* modules claiming one proto service name
//! is caught at boot, on the `try_build_state` error channel).

use r2e::prelude::*;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs.
use r2e_compile_tests::proto::ping;

#[derive(Clone)]
pub struct PingRepo;

#[bean]
impl PingRepo {
    fn new() -> Self {
        Self
    }
}

#[controller]
pub struct PingService {
    #[inject]
    repo: PingRepo,
}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        let _ = (&self.repo, request);
        unimplemented!()
    }
}

#[module(providers(PingRepo), grpc_services(PingService, PingService))]
pub struct PingModule;

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<PingModule>();
}
