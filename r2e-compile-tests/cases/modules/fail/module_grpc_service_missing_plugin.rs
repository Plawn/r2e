//! `#[module(grpc_services(...))]` implies the `GrpcServer` plugin: the macro
//! appends it to the module's `RequiredPlugins`, so registering the module on a
//! builder that never installed the plugin is a compile error **naming
//! `GrpcServer`** — not an opaque failure at serve time (the service would
//! otherwise be registered into a registry nobody drains).

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

#[module(providers(PingRepo), grpc_services(PingService))]
pub struct PingModule;

fn main() {
    // No `.plugin(GrpcServer::on_port(..))` before the module.
    let _ = r2e::AppBuilder::new().register_module::<PingModule>();
}
