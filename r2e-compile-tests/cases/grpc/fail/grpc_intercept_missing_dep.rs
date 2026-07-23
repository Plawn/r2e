//! An `#[intercept(...)]` spec on a `#[grpc_routes]` method reads a bean the
//! app never provided — must be rejected at `register_grpc_service()`.
//! gRPC interceptor sets are prebuilt from the bean context in `add_to_routes`,
//! and their `Deps` are folded into `EndpointDeps` exactly like HTTP route
//! decorator deps.

use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;
use std::future::Future;

/// The bean the interceptor needs — deliberately never provided.
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

// Real tonic-build output, compiled from `proto/ping.proto` by
// `r2e-grpc-build` in this crate's build.rs.
use r2e_compile_tests::proto::ping;

#[controller]
pub struct PingService {}

#[grpc_routes(ping::ping_server::Ping)]
impl PingService {
    #[intercept(Audit::spec())]
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<ping::PingReply>, r2e::r2e_grpc::tonic::Status>
    {
        let _ = request;
        unimplemented!()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_grpc_service::<PingService>()
    };
}
