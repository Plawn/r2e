//! A module gRPC service whose `#[intercept(...)]` spec reads a bean outside
//! the module's scope (neither provided nor imported) must be rejected at
//! `register_module` — a gRPC endpoint's decorator deps are folded into
//! `EndpointDeps::Deps` and checked module-locally, exactly like an HTTP
//! controller's guard deps.

use r2e::prelude::*;
use std::future::Future;

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs.
use r2e_compile_tests::proto::ping;

/// The bean the interceptor needs — not in the module's scope.
#[derive(Clone)]
pub struct AuditSink;

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    sink: AuditSink,
}

impl<R: Send> Interceptor<R> for Audit {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let _ = &self.sink;
        async move { next().await }
    }
}

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
    repo: PingRepo, // in scope — only the interceptor dep is not
}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    #[intercept(Audit::spec())]
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
    let _ = r2e::AppBuilder::new()
        .plugin(r2e::r2e_grpc::GrpcServer::on_port("127.0.0.1:50051"))
        .register_module::<PingModule>();
}
