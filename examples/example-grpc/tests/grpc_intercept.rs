//! Runtime proof for graph-built gRPC interceptors: `#[intercept(...)]`
//! sites on gRPC methods are prebuilt ONCE at registration (`add_to_routes`)
//! from the resolved bean context — the same `DecoratorSpec::build` path as
//! HTTP route interceptors — so bean-reading specs work on gRPC methods.

use std::sync::{Arc, Mutex};

use r2e::prelude::*;
use r2e::r2e_grpc::GrpcService;

pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::greeter_client::GreeterClient;
use proto::greeter::{HelloReply, HelloRequest};

// ── Bean + graph-built interceptor ──────────────────────────────────────

#[derive(Clone, Default)]
pub struct CallLog(pub Arc<Mutex<Vec<String>>>);

#[derive(DecoratorBean)]
pub struct LogCalls {
    #[inject]
    log: CallLog,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for LogCalls {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.tag, method_name));
            next().await
        }
    }
}

// ── Service: one intercepted method, one plain ──────────────────────────

#[controller]
pub struct TestGreeter {}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl TestGreeter {
    #[intercept(LogCalls::spec("grpc"))]
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("hi {}", request.get_ref().name),
        }))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("admin {}", request.get_ref().name),
        }))
    }
}

// ── Test ────────────────────────────────────────────────────────────────

#[r2e::test]
async fn grpc_interceptor_is_built_from_the_bean_graph() {
    let log = CallLog::default();
    let builder = AppBuilder::new().provide(log.clone()).build_state().await;

    // Registration path: the interceptor set is built here, once, from the
    // retained bean graph.
    let routes = TestGreeter::add_to_routes(
        tonic::service::Routes::default(),
        builder.bean_context(),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_routes(routes)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let mut client = GreeterClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    for name in ["a", "b"] {
        let resp = client
            .say_hello(HelloRequest { name: name.into() })
            .await
            .unwrap();
        assert_eq!(resp.get_ref().message, format!("hi {name}"));
    }
    // The uninstrumented method must not hit the interceptor.
    client
        .say_hello_admin(HelloRequest { name: "c".into() })
        .await
        .unwrap();

    let entries = log.0.lock().unwrap().clone();
    assert_eq!(entries, vec!["grpc:say_hello", "grpc:say_hello"]);
}
