//! Scaffolding for `llm/grpc.md`.
//!
//! `llm-doctests` has no build script compiling `.proto` files, so `proto`
//! below is a hand-written stand-in with exactly the shape `r2e-grpc-build`
//! emits (a `<service>_server` module holding `SERVICE_NAME`, the async
//! service trait and the `…Server<T>` tower service) — enough for
//! `#[grpc_routes]` to expand and typecheck against it.


pub use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer, GrpcService};
pub use r2e::{BeanContext, EndpointDeps};
pub use std::sync::Arc;

/// The service the controller injects.
#[derive(Clone)]
pub struct UserService;

/// Stand-in for the module `r2e::r2e_grpc::include_protos!()` expands to.
pub mod proto {
    /// Combined descriptor set, for `with_reflection()`.
    pub const FILE_DESCRIPTOR_SET: &[u8] = &[];

    /// One module per proto package (`package greeter;`).
    pub mod greeter {
        #[derive(Clone, Default)]
        pub struct HelloRequest {
            pub name: String,
        }

        #[derive(Clone, Default)]
        pub struct HelloReply {
            pub message: String,
        }

        /// What `tonic-prost-build` writes for `service Greeter { … }`.
        pub mod greeter_server {
            use super::{HelloReply, HelloRequest};
            use r2e::r2e_grpc::tonic;

            pub const SERVICE_NAME: &str = "greeter.Greeter";

            #[tonic::async_trait]
            pub trait Greeter: Send + Sync + 'static {
                async fn say_hello(
                    &self,
                    request: tonic::Request<HelloRequest>,
                ) -> Result<tonic::Response<HelloReply>, tonic::Status>;
            }

            pub struct GreeterServer<T> {
                inner: std::sync::Arc<T>,
            }

            impl<T> GreeterServer<T> {
                pub fn new(inner: T) -> Self {
                    Self {
                        inner: std::sync::Arc::new(inner),
                    }
                }
            }

            impl<T> Clone for GreeterServer<T> {
                fn clone(&self) -> Self {
                    Self {
                        inner: std::sync::Arc::clone(&self.inner),
                    }
                }
            }

            impl<T> tonic::server::NamedService for GreeterServer<T> {
                const NAME: &'static str = SERVICE_NAME;
            }

            impl<T: Greeter> tower::Service<http::Request<tonic::body::Body>> for GreeterServer<T> {
                type Response = http::Response<tonic::body::Body>;
                type Error = std::convert::Infallible;
                type Future = std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
                >;

                fn poll_ready(
                    &mut self,
                    _cx: &mut std::task::Context<'_>,
                ) -> std::task::Poll<Result<(), Self::Error>> {
                    std::task::Poll::Ready(Ok(()))
                }

                fn call(&mut self, _req: http::Request<tonic::body::Body>) -> Self::Future {
                    Box::pin(async { Ok(http::Response::new(tonic::body::Body::empty())) })
                }
            }
        }
    }
}

pub use proto::greeter::{HelloReply, HelloRequest};

/// The service the registration snippet registers — the two impls
/// `#[grpc_routes]` writes, spelled out so the snippet needs no generated
/// proto module.
pub struct GreeterService;

impl EndpointDeps for GreeterService {
    type Deps = r2e::type_list::TNil;
}

impl GrpcService for GreeterService {
    fn service_name() -> &'static str {
        proto::greeter::greeter_server::SERVICE_NAME
    }

    fn add_to_routes(
        routes: r2e::r2e_grpc::tonic::service::Routes,
        _ctx: &Arc<BeanContext>,
    ) -> r2e::r2e_grpc::tonic::service::Routes {
        routes
    }
}
