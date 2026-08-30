//! Happy path for `#[module(grpc_services(...))]`: a vertical slice owns its
//! gRPC service, the service injects a **private** provider (never exported,
//! so absent from the application state), and the module is registered on a
//! builder that installed the `GrpcServer` plugin.

use r2e::prelude::*;
use r2e::r2e_grpc::GrpcServer;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs.
use r2e_compile_tests::proto::ping;

/// Module-private: neither exported nor visible from the app state.
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
        let _ = &self.repo;
        Ok(r2e::r2e_grpc::tonic::Response::new(ping::PingReply {
            msg: request.into_inner().msg,
        }))
    }
}

#[module(providers(PingRepo), grpc_services(PingService))]
pub struct PingModule;

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .plugin(GrpcServer::on_port("127.0.0.1:50051"))
            .register_module::<PingModule>()
            .build_state()
            .await
    };
}
