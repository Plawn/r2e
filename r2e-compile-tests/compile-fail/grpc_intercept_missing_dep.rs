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

#[controller]
pub struct PingService {}

#[grpc_routes(proto::ping_server::Ping)]
impl PingService {
    #[intercept(Audit::spec())]
    async fn ping(
        &self,
        request: r2e::r2e_grpc::tonic::Request<()>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<()>, r2e::r2e_grpc::tonic::Status> {
        let _ = request;
        unimplemented!()
    }
}

// Minimal hand-written stand-in for tonic-build output: just enough surface
// for the generated wrapper (tonic trait impl + `Server::add_service`) to
// typecheck without a proto file.
mod proto {
    pub mod ping_server {
        use r2e::r2e_grpc::tonic;

        // tonic-build emits this module-level const; the `#[grpc_routes]`
        // codegen references `<module>::SERVICE_NAME`, so the stand-in must
        // provide it too.
        pub const SERVICE_NAME: &str = "test.Ping";

        #[tonic::async_trait]
        pub trait Ping: Send + Sync + 'static {
            async fn ping(
                &self,
                request: tonic::Request<()>,
            ) -> Result<tonic::Response<()>, tonic::Status>;
        }

        #[derive(Clone)]
        pub struct PingServer<T>(std::sync::Arc<T>);

        impl<T> PingServer<T> {
            pub fn new(inner: T) -> Self {
                Self(std::sync::Arc::new(inner))
            }
        }

        impl<T: Ping> tonic::codegen::Service<tonic::codegen::http::Request<tonic::body::Body>>
            for PingServer<T>
        {
            type Response = tonic::codegen::http::Response<tonic::body::Body>;
            type Error = std::convert::Infallible;
            type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;

            fn poll_ready(
                &mut self,
                _cx: &mut tonic::codegen::Context<'_>,
            ) -> tonic::codegen::Poll<Result<(), Self::Error>> {
                tonic::codegen::Poll::Ready(Ok(()))
            }

            fn call(
                &mut self,
                _req: tonic::codegen::http::Request<tonic::body::Body>,
            ) -> Self::Future {
                unimplemented!()
            }
        }

        impl<T> tonic::server::NamedService for PingServer<T> {
            const NAME: &'static str = "test.Ping";
        }
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
