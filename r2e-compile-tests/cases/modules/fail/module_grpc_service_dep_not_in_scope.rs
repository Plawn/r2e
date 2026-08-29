//! A module-owned gRPC service is dependency-checked **module-locally** at
//! `register_module`, exactly like a module controller: injecting a bean that
//! is neither provided by the module nor imported is a compile error there,
//! even though the app itself provides it.

use r2e::prelude::*;
use r2e::r2e_grpc::GrpcServer;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs.
use r2e_compile_tests::proto::ping;

/// Provided by the app, but NOT imported by the module.
#[derive(Clone)]
pub struct Pool;

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
    #[inject]
    pool: Pool, // outside the module's scope
}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status> {
        let _ = (&self.repo, &self.pool, request);
        unimplemented!()
    }
}

#[module(providers(PingRepo), grpc_services(PingService))]
pub struct PingModule;

fn main() {
    let _ = r2e::AppBuilder::new()
        .plugin(GrpcServer::on_port("127.0.0.1:50051"))
        .provide(Pool)
        .register_module::<PingModule>();
}
