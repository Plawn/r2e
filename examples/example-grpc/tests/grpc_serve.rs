//! e2e proof that `AppBuilder::serve()` actually starts the gRPC server
//! (DI backlog item 12): services registered with `register_grpc_service`
//! are drained from the `GrpcServiceRegistry` at serve time and served —
//! on a separate port (`GrpcServer::on_port`) or multiplexed with HTTP on
//! one port (`GrpcServer::multiplexed`) — with graph-built interceptors
//! running on the calls.
//!
//! Shutdown: the servers are stopped programmatically via
//! [`StopHandle`](r2e::prelude::StopHandle) (`prepare()` → `stop_handle()` →
//! `run()`), which exercises the real graceful-shutdown path — including the
//! awaited gRPC drain on the separate-port transport.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use r2e::prelude::*;
use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod proto {
    tonic::include_proto!("greeter");
}

use proto::greeter_client::GreeterClient;
use proto::{HelloReply, HelloRequest};

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

// ── gRPC service + HTTP controller ──────────────────────────────────────

#[controller]
pub struct TestGreeter {}

#[grpc_routes(proto::greeter_server::Greeter)]
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

#[controller(path = "/api")]
pub struct PingController;

#[routes]
impl PingController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Pick a free TCP port (bind to :0, read the port, release). The tiny
/// reuse window before the server binds it is acceptable for tests.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Stop the server via its `StopHandle` and assert `run()` terminated
/// cleanly — the real graceful-shutdown path, including the awaited gRPC
/// drain on the separate-port transport.
async fn stop_and_await_clean(
    stop: r2e::prelude::StopHandle,
    server: tokio::task::JoinHandle<Result<(), String>>,
) {
    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s after StopHandle::stop()")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");
}

/// Connect a gRPC client with a retry deadline, so the test fails with a
/// clear panic (instead of hanging) when the server never comes up — which
/// is exactly what happened before the serve path was wired.
async fn connect_client(port: u16) -> GreeterClient<tonic::transport::Channel> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match GreeterClient::connect(format!("http://127.0.0.1:{port}")).await {
            Ok(client) => return client,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "gRPC server did not come up on port {port}: {e}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[r2e::test]
async fn serve_starts_grpc_on_separate_port() {
    let grpc_port = free_port();
    let http_port = free_port();
    let log = CallLog::default();

    // The REAL path: plugin → build_state → register_grpc_service → serve().
    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port(format!("127.0.0.1:{grpc_port}")))
        .provide(log.clone())
        .build_state()
        .await
        .register_grpc_service::<TestGreeter>();

    let prepared = app.prepare(&format!("127.0.0.1:{http_port}"));
    let stop = prepared.stop_handle();
    let server = tokio::spawn(async move {
        prepared.run().await.map_err(|e| e.to_string())
    });

    let mut client = connect_client(grpc_port).await;

    let resp = client
        .say_hello(HelloRequest { name: "e2e".into() })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hi e2e");

    // The uninstrumented method must not hit the interceptor.
    let resp = client
        .say_hello_admin(HelloRequest { name: "x".into() })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "admin x");

    // The graph-built interceptor ran on the intercepted method only.
    let entries = log.0.lock().unwrap().clone();
    assert_eq!(entries, vec!["grpc:say_hello"]);

    // Programmatic graceful stop: run() resolves cleanly once the HTTP
    // listener AND the gRPC server (tracked drain) have shut down.
    stop_and_await_clean(stop, server).await;
}

#[r2e::test]
async fn serve_multiplexes_grpc_and_http_on_one_port() {
    let port = free_port();
    let log = CallLog::default();

    let app = AppBuilder::new()
        .plugin(GrpcServer::multiplexed())
        .provide(log.clone())
        .build_state()
        .await
        .register_grpc_service::<TestGreeter>()
        .register_controller::<PingController>();

    let prepared = app.prepare(&format!("127.0.0.1:{port}"));
    let stop = prepared.stop_handle();
    let server = tokio::spawn(async move {
        prepared.run().await.map_err(|e| e.to_string())
    });

    // gRPC on the HTTP port (content-type routed, h2c prior knowledge).
    let mut client = connect_client(port).await;
    let resp = client
        .say_hello(HelloRequest { name: "mux".into() })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hi mux");

    let entries = log.0.lock().unwrap().clone();
    assert_eq!(entries, vec!["grpc:say_hello"]);

    // Plain HTTP/1.1 on the same port still reaches the axum router.
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET /api/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected HTTP response: {response}"
    );
    assert!(response.contains("pong"), "unexpected HTTP body: {response}");

    stop_and_await_clean(stop, server).await;
}
