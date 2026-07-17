// Canonical example-grpc application source.
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// includes the same file directly in the binary tip crate for production and
// real Subsecond hot-patching.

use r2e::prelude::*;
use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer};

pub mod proto {
    tonic::include_proto!("greeter");

    /// Encoded `FileDescriptorSet` emitted by `build.rs`
    /// (`file_descriptor_set_path`), consumed by gRPC server reflection.
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("greeter_descriptor");
}

use proto::{HelloReply, HelloRequest};

// ── State ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct GreetingPrefix(pub String);

/// Bean read by the gRPC call-log interceptor.
#[derive(Clone, Default)]
pub struct CallLog(pub std::sync::Arc<std::sync::Mutex<Vec<String>>>);

// ── Interceptor built from the bean graph ──────────────────────────────
//
// gRPC `#[intercept(...)]` sites are prebuilt once at registration
// (`add_to_routes`), from the resolved bean context — same `DecoratorSpec`
// path as HTTP route interceptors, so bean-reading specs work here too.

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

// ── gRPC Service ───────────────────────────────────────────────────────

#[controller]
pub struct GreeterService {
    #[inject]
    greeting_prefix: GreetingPrefix,
}

#[grpc_routes(proto::greeter_server::Greeter, descriptor = proto::FILE_DESCRIPTOR_SET)]
impl GreeterService {
    #[intercept(LogCalls::spec("grpc"))]
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("{} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("[ADMIN] {} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }
}

// ── HTTP Controller (to show multiplexing) ─────────────────────────────

#[controller(path = "/api")]
pub struct HealthController;

#[routes]
impl HealthController {
    #[get("/health")]
    async fn health(&self) -> &'static str {
        "OK"
    }
}

// ── Application blueprint ──────────────────────────────────────────────

/// The canonical application blueprint. Serves gRPC on :50051 (via the
/// `GrpcServer` plugin) multiplexed alongside HTTP on :3000 (the `serve_auto`
/// default that `launch` uses).
pub struct GrpcApp;

impl App for GrpcApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        // Reflection serves the descriptor set collected from the services
        // above: `grpcurl -plaintext localhost:50051 list` works out of the box.
        b.plugin(GrpcServer::on_port("0.0.0.0:50051").with_reflection())
            .provide(GreetingPrefix("Hello".to_string()))
            .provide(CallLog::default())
            .build_state()
            .await
            .on_start(|_state| async move {
                tracing::info!("HTTP on :3000, gRPC on :50051");
                Ok(())
            })
            .register_grpc_service::<GreeterService>()
            .register_controller::<HealthController>()
    }
}
