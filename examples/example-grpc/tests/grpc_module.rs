//! e2e proof for ticket #989: a feature module owns its gRPC service exactly
//! like its HTTP controllers.
//!
//! `#[module(grpc_services(...))]` registers the service from the retained
//! bean context at `build_state()`, so the service injects the module's
//! **private** providers — beans that are absent from the application state
//! and therefore out of reach of the app-level `register_grpc_service`.
//! The service is then served for real (`GrpcServer::on_port` + `serve`) and
//! called over the wire, alongside the module's HTTP controller.

mod common;

use std::sync::{Arc, Mutex};

use common::{connect_channel, free_port, stop_and_await_clean};
use r2e::prelude::*;
use r2e::r2e_grpc::GrpcServer;

pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::greeter_client::GreeterClient;
use proto::greeter::{HelloReply, HelloRequest};

// ── Module-private providers ────────────────────────────────────────────
//
// Neither is exported: they exist only inside the module's subgraph.

#[derive(Clone)]
pub struct GreetingRepo {
    prefix: &'static str,
}

#[bean]
impl GreetingRepo {
    fn new() -> Self {
        Self { prefix: "Bonjour" }
    }
}

#[derive(Clone)]
pub struct GreetingService {
    repo: GreetingRepo,
}

#[bean]
impl GreetingService {
    fn new(repo: GreetingRepo) -> Self {
        Self { repo }
    }

    fn greet(&self, name: &str) -> String {
        format!("{} {}!", self.repo.prefix, name)
    }
}

// ── App-level bean the module imports (proves imports still work) ───────

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

// ── The module's gRPC service: injects a PRIVATE provider ───────────────

#[controller]
pub struct ModuleGreeter {
    #[inject]
    service: GreetingService,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl ModuleGreeter {
    // The interceptor spec reads `CallLog`, an *imported* bean — decorator
    // deps are folded into the endpoint's `EndpointDeps::Deps`, so they are
    // module-scope checked too.
    #[intercept(LogCalls::spec("module"))]
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: self.service.greet(&request.get_ref().name),
        }))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("[ADMIN] {}", self.service.greet(&request.get_ref().name)),
        }))
    }
}

// ── The module's HTTP controller: same private beans ────────────────────

#[controller(path = "/greeting")]
pub struct GreetingController {
    #[inject]
    service: GreetingService,
}

#[routes]
impl GreetingController {
    #[get("/{name}")]
    async fn greet(&self, Path(name): Path<String>) -> String {
        self.service.greet(&name)
    }
}

// ── The vertical slice ──────────────────────────────────────────────────
//
// `grpc_services(..)` also adds `GrpcServer` to the module's required
// plugins, so omitting `.plugin(GrpcServer::..)` is a compile error naming
// it (see `r2e-compile-tests/cases/modules/fail/`).

#[module(
    providers(GreetingRepo, GreetingService),
    grpc_services(ModuleGreeter),
    controllers(GreetingController),
    imports(CallLog)
)]
pub struct GreetingModule;

// ── Tests ───────────────────────────────────────────────────────────────

#[r2e::test]
async fn a_module_grpc_service_is_served_and_injects_private_beans() {
    let grpc_port = free_port();
    let http_port = free_port();
    let log = CallLog::default();

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port(format!("127.0.0.1:{grpc_port}")))
        .provide(log.clone())
        .register_module::<GreetingModule>()
        .build_state()
        .await;

    let prepared = app.prepare(&format!("127.0.0.1:{http_port}"));
    let stop = prepared.stop_handle();
    let server = r2e::rt::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });

    let mut client = GreeterClient::new(connect_channel(grpc_port).await);

    // The module's gRPC service is on the wire, answering from beans that
    // exist only inside the module.
    let resp = client
        .say_hello(HelloRequest {
            name: "module".into(),
        })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "Bonjour module!");

    let resp = client
        .say_hello_admin(HelloRequest {
            name: "root".into(),
        })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "[ADMIN] Bonjour root!");

    // The graph-built interceptor ran, on the intercepted method only.
    assert_eq!(log.0.lock().unwrap().clone(), vec!["module:say_hello"]);

    // The module's HTTP controller shares the very same private beans.
    let body = http_get(http_port, "/greeting/http").await;
    assert_eq!(body, "Bonjour http!");

    stop_and_await_clean(stop, server).await;
}

/// Minimal HTTP/1.1 GET against the test server (same raw-socket approach as
/// `grpc_serve.rs` — this example has no HTTP client dependency).
async fn http_get(port: u16, path: &str) -> String {
    use r2e::rt::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = r2e::rt::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf).to_string();
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected HTTP response: {response}"
    );
    response
        .split("\r\n\r\n")
        .nth(1)
        .expect("no body")
        .trim()
        .to_string()
}
