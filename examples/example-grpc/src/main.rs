use r2e::prelude::*;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

pub mod proto {
    tonic::include_proto!("greeter");
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

#[grpc_routes(proto::greeter_server::Greeter)]
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

// ── Main ───────────────────────────────────────────────────────────────

#[r2e::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = GreetingPrefix("Hello".to_string());

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .provide(prefix)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_grpc_service::<GreeterService>()
        .register_controller::<HealthController>();

    tracing::info!("HTTP on :3000, gRPC on :50051");
    app.serve("0.0.0.0:3000").await
}
