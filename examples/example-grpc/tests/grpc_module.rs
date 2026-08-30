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

// ── One owner per service (review round 1) ──────────────────────────────
//
// A gRPC service name may be registered exactly once. Two Rust types can
// implement the *same* proto service, which is how a duplicate reaches the
// registry in practice — the wire name (`greeter.Greeter`) collides even
// though the types do not.

/// Private provider of the rival slice.
#[derive(Clone)]
pub struct RivalGreeting;

#[bean]
impl RivalGreeting {
    fn new() -> Self {
        Self
    }
}

#[controller]
pub struct RivalGreeter {
    #[inject]
    rival: RivalGreeting,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl RivalGreeter {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let _ = &self.rival;
        Ok(tonic::Response::new(HelloReply {
            message: format!("rival {}", request.get_ref().name),
        }))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let _ = &self.rival;
        Ok(tonic::Response::new(HelloReply {
            message: format!("rival admin {}", request.get_ref().name),
        }))
    }
}

#[module(providers(RivalGreeting), grpc_services(RivalGreeter))]
pub struct RivalModule;

/// Same wire name again, but with app-scoped deps — so it is registrable
/// through the app-level `.register_grpc_service::<_>()`.
#[controller]
pub struct AppGreeter {
    #[inject]
    log: CallLog,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl AppGreeter {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let _ = &self.log;
        Ok(tonic::Response::new(HelloReply {
            message: format!("app {}", request.get_ref().name),
        }))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let _ = &self.log;
        Ok(tonic::Response::new(HelloReply {
            message: format!("app admin {}", request.get_ref().name),
        }))
    }
}

#[r2e::test]
async fn two_modules_claiming_one_service_name_fail_the_build() {
    use r2e::beans::BeanError;

    let err = AppBuilder::new()
        .plugin(GrpcServer::on_port("127.0.0.1:0"))
        .provide(CallLog::default())
        .register_module::<GreetingModule>()
        .register_module::<RivalModule>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("two modules registering `greeter.Greeter` must not both succeed");

    match &err {
        BeanError::DuplicateEndpoint {
            endpoint,
            module,
            name,
        } => {
            assert!(
                endpoint.contains("RivalGreeter"),
                "the rejected endpoint must be named: {endpoint}"
            );
            assert!(
                module.contains("RivalModule"),
                "the declaring module must be named: {module}"
            );
            assert_eq!(*name, "greeter.Greeter");
        }
        other => panic!("expected DuplicateEndpoint, got {other:?}"),
    }

    let rendered = err.to_string();
    assert!(rendered.contains("greeter.Greeter"), "{rendered}");
    assert!(rendered.contains("already registered"), "{rendered}");
}

#[r2e::test]
async fn an_app_level_registration_of_a_module_owned_service_panics() {
    use r2e::r2e_grpc::AppBuilderGrpcExt;

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("127.0.0.1:0"))
        .provide(CallLog::default())
        .register_module::<GreetingModule>()
        .build_state()
        .await;

    // Modules register inside `build_state()`, so the app-level call is the
    // second one — and the one that must refuse.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        app.register_grpc_service::<AppGreeter>()
    }));
    std::panic::set_hook(previous);

    let payload = outcome.err().expect("the duplicate registration must panic");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("panic payload should be a string");
    assert!(
        message.contains("greeter.Greeter") && message.contains("already registered"),
        "the panic must name the clashing service: {message}"
    );
}

// ── Boot-error channel: a module endpoint's declared config ─────────────

#[controller]
pub struct ConfiguredGreeter {
    #[config("greeter.api-key")]
    api_key: String,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl ConfiguredGreeter {
    async fn say_hello(
        &self,
        _request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: self.api_key.clone(),
        }))
    }

    async fn say_hello_admin(
        &self,
        _request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: self.api_key.clone(),
        }))
    }
}

#[module(grpc_services(ConfiguredGreeter), imports(R2eConfig))]
pub struct ConfiguredModule;

#[r2e::test]
async fn a_module_endpoint_with_missing_config_fails_the_build() {
    use r2e::beans::BeanError;

    let err = AppBuilder::new()
        .plugin(GrpcServer::on_port("127.0.0.1:0"))
        .register_module::<ConfiguredModule>()
        .override_config(r2e::config::R2eConfig::empty())
        .load_config::<()>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("the endpoint declares a key the config does not have");

    assert!(
        matches!(err, BeanError::EndpointConfig { .. }),
        "expected EndpointConfig, got {err:?}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains("ConfiguredGreeter"),
        "the endpoint must be named: {rendered}"
    );
    assert!(
        rendered.contains("CONFIGURATION ERRORS"),
        "the aggregated report must survive: {rendered}"
    );
    assert!(
        rendered.contains("greeter.api-key"),
        "the missing key must be listed: {rendered}"
    );
}

// ── Boot-error channel: the missing-plugin backstop ─────────────────────
//
// `#[module(grpc_services(..))]` appends `GrpcServer` to `RequiredPlugins`,
// and `GrpcMarker` cannot be constructed outside r2e-grpc, so the macro path
// really cannot reach `build_state()` without the plugin. A hand-written
// `FeatureModule` impl can skip `RequiredPlugins` — this is that backstop.

struct HandWrittenModule;

impl FeatureModule for HandWrittenModule {
    type Providers = r2e::type_list::TCons<RivalGreeting, r2e::type_list::TNil>;
    type Controllers = ();
    type Exports = r2e::type_list::TNil;
    type Imports = r2e::type_list::TNil;
    type RequiredPlugins = (); // deliberately NOT declaring GrpcServer
    type Plugins = ();
    type Endpoints = r2e::r2e_grpc::ModuleGrpcServices<(RivalGreeter,)>;

    fn plugins() {}
}

#[r2e::test]
async fn a_module_endpoint_without_its_transport_plugin_fails_the_build() {
    use r2e::beans::BeanError;

    let err = AppBuilder::new()
        .register_module::<HandWrittenModule>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("no GrpcServer plugin means no registry to register into");

    match &err {
        BeanError::MissingTransportPlugin {
            endpoint,
            module,
            plugin,
        } => {
            assert!(endpoint.contains("RivalGreeter"), "{endpoint}");
            assert!(module.contains("HandWrittenModule"), "{module}");
            assert_eq!(*plugin, "GrpcServer");
        }
        other => panic!("expected MissingTransportPlugin, got {other:?}"),
    }

    let rendered = err.to_string();
    assert!(rendered.contains("GrpcServer"), "{rendered}");
    assert!(rendered.contains("HandWrittenModule"), "{rendered}");
}
