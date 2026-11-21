//! `app.grpc()` — the generated tonic clients against a `TestApp`, with no
//! socket: the same service set a transport would serve (reflection included),
//! on both `GrpcServer` transports, with guards, identity and interceptors
//! running as they do over the wire.

use std::pin::Pin;
use std::sync::Arc;

use example_grpc::proto::greeter::greeter_client::GreeterClient;
use example_grpc::proto::greeter::{HelloReply, HelloRequest};
use example_grpc::proto::ticker::ticker_client::TickerClient;
use example_grpc::proto::ticker::{Tick, WatchRequest};
use example_grpc::{proto, CallLog, GrpcApp};
use r2e::prelude::*;
use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer};
use r2e::r2e_security::AuthenticatedUser;
use r2e::rt::stream::StreamExt;
use r2e_test::{TestApp, TestJwt};
use tonic_reflection::pb::v1::server_reflection_client::ServerReflectionClient;
use tonic_reflection::pb::v1::server_reflection_request::MessageRequest;
use tonic_reflection::pb::v1::server_reflection_response::MessageResponse;
use tonic_reflection::pb::v1::ServerReflectionRequest;

fn hello(name: &str) -> HelloRequest {
    HelloRequest { name: name.into() }
}

// ── The canonical app: separate port, never bound under `TestApp` ───────

#[r2e::test]
async fn the_canonical_app_answers_without_a_socket() {
    let app = TestApp::boot::<GrpcApp>().await;
    let mut greeter = GreeterClient::new(app.grpc());

    let reply = greeter.say_hello(hello("Bob")).await.unwrap();
    assert_eq!(reply.get_ref().message, "Hello Bob!");

    // The `#[intercept]` site ran, on the very bean graph the test reads.
    let log = app.bean::<CallLog>();
    assert_eq!(*log.0.lock().unwrap(), vec!["grpc:say_hello".to_string()]);
}

#[r2e::test]
async fn the_channel_serves_what_the_transport_would() {
    let app = TestApp::boot::<GrpcApp>().await;
    let grpc = app.grpc();
    assert_eq!(
        grpc.service_names(),
        [
            "greeter.Greeter",
            "grpc.reflection.v1.ServerReflection",
            "grpc.reflection.v1alpha.ServerReflection",
        ]
    );

    // Reflection is a bidirectional stream: it runs in-process too.
    let mut reflection = ServerReflectionClient::new(grpc);
    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(MessageRequest::ListServices(String::new())),
    };
    let mut responses = reflection
        .server_reflection_info(r2e::rt::stream::iter([request]))
        .await
        .unwrap()
        .into_inner();
    let response = responses.next().await.unwrap().unwrap();
    let Some(MessageResponse::ListServicesResponse(list)) = response.message_response else {
        panic!("expected a ListServicesResponse, got {response:?}");
    };
    assert!(list.service.iter().any(|s| s.name == "greeter.Greeter"));
}

#[r2e::test]
async fn each_boot_has_its_own_services() {
    let first = TestApp::boot::<GrpcApp>().await;
    let second = TestApp::boot::<GrpcApp>().await;
    GreeterClient::new(first.grpc())
        .say_hello(hello("a"))
        .await
        .unwrap();

    assert_eq!(first.bean::<CallLog>().0.lock().unwrap().len(), 1);
    assert!(second.bean::<CallLog>().0.lock().unwrap().is_empty());
}

// ── A secured app on the multiplexed transport ──────────────────────────

#[controller]
pub struct SecuredGreeter {}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl SecuredGreeter {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
        #[inject(identity)] user: Option<AuthenticatedUser>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let who = user.map_or_else(|| "anon".to_string(), |u| u.sub().to_string());
        let tag = request
            .metadata()
            .get("x-tag")
            .map_or("-", |v| v.to_str().unwrap());
        Ok(tonic::Response::new(HelloReply {
            message: format!("hi {} from {who} [{tag}]", request.get_ref().name),
        }))
    }

    #[roles("admin")]
    async fn say_hello_admin(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("admin {} for {}", user.sub(), request.get_ref().name),
        }))
    }
}

#[controller]
pub struct SecuredTicker {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[grpc_routes(proto::ticker::ticker_server::Ticker)]
impl SecuredTicker {
    type WatchStream =
        Pin<Box<dyn r2e::rt::stream::Stream<Item = Result<Tick, tonic::Status>> + Send>>;

    async fn watch(
        &self,
        request: tonic::Request<WatchRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let subject = self.user.sub().to_string();
        let ticks = (1..=request.get_ref().count).map(move |seq| {
            Ok(Tick {
                seq,
                subject: subject.clone(),
            })
        });
        let stream: Self::WatchStream = Box::pin(r2e::rt::stream::iter(ticks));
        Ok(tonic::Response::new(stream))
    }
}

#[controller(path = "/ping")]
pub struct PingController;

#[routes]
impl PingController {
    #[get("/")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

pub struct SecuredApp;

impl App for SecuredApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        // Stands in for the production validator; `TestApp::boot` pins the
        // `TestJwt`'s over it, which is what `as_user` relies on.
        let production = TestJwt::with_config(b"production-secret", "prod", "prod");
        Ok(b
            .plugin(GrpcServer::multiplexed())
            .provide(Arc::new(production.claims_validator()))
            .build_state()
            .await
            .register_grpc_service::<SecuredGreeter>()
            .register_grpc_service::<SecuredTicker>()
            .register_controller::<PingController>())
    }
}

#[r2e::test]
async fn as_user_authenticates_the_way_http_does() {
    let app = TestApp::boot::<SecuredApp>().await;
    let grpc = app.grpc();
    let mut anonymous = GreeterClient::new(grpc.clone());
    let mut viewer = GreeterClient::new(grpc.clone().as_user("bob", &["viewer"]));
    let mut admin = GreeterClient::new(grpc.as_user("alice", &["admin"]));

    let reply = anonymous.say_hello(hello("a")).await.unwrap();
    assert_eq!(reply.get_ref().message, "hi a from anon [-]");
    let reply = viewer.say_hello(hello("a")).await.unwrap();
    assert_eq!(reply.get_ref().message, "hi a from bob [-]");

    let err = anonymous.say_hello_admin(hello("b")).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    let err = viewer.say_hello_admin(hello("b")).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    let reply = admin.say_hello_admin(hello("b")).await.unwrap();
    assert_eq!(reply.get_ref().message, "admin alice for b");
}

#[r2e::test]
async fn a_bearer_token_and_metadata_ride_every_call() {
    let app = TestApp::boot::<SecuredApp>().await;
    let token = app.test_jwt().token("carol", &[]);
    let mut greeter = GreeterClient::new(app.grpc().bearer(&token).metadata("x-tag", "channel"));

    let reply = greeter.say_hello(hello("a")).await.unwrap();
    assert_eq!(reply.get_ref().message, "hi a from carol [channel]");

    // What a call sets itself wins over the channel's metadata.
    let mut request = tonic::Request::new(hello("a"));
    request.metadata_mut().insert("x-tag", "call".parse().unwrap());
    let reply = greeter.say_hello(request).await.unwrap();
    assert_eq!(reply.get_ref().message, "hi a from carol [call]");

    // A presented-and-invalid token is rejected, as over the wire.
    let mut garbage = GreeterClient::new(app.grpc().bearer("garbage"));
    let err = garbage.say_hello(hello("a")).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[r2e::test]
async fn a_server_stream_is_read_to_its_end() {
    let app = TestApp::boot::<SecuredApp>().await;
    let mut ticker = TickerClient::new(app.grpc().as_user("alice", &[]));

    let ticks: Vec<Tick> = ticker
        .watch(WatchRequest { count: 3 })
        .await
        .unwrap()
        .into_inner()
        .map(|tick| tick.unwrap())
        .collect()
        .await;
    assert_eq!(ticks.iter().map(|t| t.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(ticks.iter().all(|t| t.subject == "alice"));

    // Rejected at the head: the call fails before a stream exists.
    let err = TickerClient::new(app.grpc())
        .watch(WatchRequest { count: 3 })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[r2e::test]
async fn http_and_grpc_share_one_multiplexed_app() {
    let app = TestApp::boot::<SecuredApp>().await;
    assert_eq!(app.grpc().service_names(), ["greeter.Greeter", "ticker.Ticker"]);

    app.get("/ping").send().await.assert_ok();
    let reply = GreeterClient::new(app.grpc())
        .say_hello(hello("a"))
        .await
        .unwrap();
    assert_eq!(reply.get_ref().message, "hi a from anon [-]");
}

// ── Misuse fails with a message that says what to do ────────────────────

pub struct NoServicesApp;

impl App for NoServicesApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok(b
            .plugin(GrpcServer::on_port("127.0.0.1:0"))
            .build_state()
            .await
            .register_controller::<PingController>())
    }
}

#[r2e::test]
#[should_panic(expected = "no gRPC service is registered")]
async fn no_registered_service_panics() {
    let app = TestApp::boot::<NoServicesApp>().await;
    let _ = app.grpc();
}
