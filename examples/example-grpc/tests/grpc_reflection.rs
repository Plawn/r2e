//! e2e proof that `GrpcServer::with_reflection()` installs a real reflection
//! service (task #653): a `grpcurl`-style list-services call succeeds against
//! a served app on both transports (separate port + multiplexed), and the
//! descriptor set declared via `#[grpc_routes(..., descriptor = ...)]`
//! resolves symbol lookups. Both protocol versions (v1 + v1alpha) are served.

mod common;

use common::{connect_channel, free_port, stop_and_await_clean};
use r2e::prelude::*;
use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer};
use tonic_reflection::pb::v1::server_reflection_client::ServerReflectionClient;
use tonic_reflection::pb::v1::server_reflection_request::MessageRequest;
use tonic_reflection::pb::v1::server_reflection_response::MessageResponse;
use tonic_reflection::pb::v1::ServerReflectionRequest;

pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::{HelloReply, HelloRequest};

#[controller]
pub struct TestGreeter {}

#[grpc_routes(proto::greeter::greeter_server::Greeter, descriptor = proto::FILE_DESCRIPTOR_SET)]
impl TestGreeter {
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

// ── Helpers ─────────────────────────────────────────────────────────────

/// Send one v1 reflection request and return the single response message.
async fn reflect_v1(
    channel: tonic::transport::Channel,
    request: MessageRequest,
) -> MessageResponse {
    let mut client = ServerReflectionClient::new(channel);
    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(request),
    };
    let mut stream = client
        .server_reflection_info(tokio_stream::iter(vec![request]))
        .await
        .expect("ServerReflectionInfo call failed")
        .into_inner();
    stream
        .message()
        .await
        .expect("reflection stream errored")
        .expect("reflection stream closed without a response")
        .message_response
        .expect("reflection response carried no message")
}

/// List services via v1 reflection and assert the user service and the v1
/// reflection service are advertised. (Each protocol version's builder
/// registers only its own reflection descriptor, so the v1 listing does not
/// carry `grpc.reflection.v1alpha.ServerReflection` — the v1alpha listing,
/// asserted separately below, carries the user services plus its own.)
async fn assert_list_services(channel: tonic::transport::Channel) {
    let response = reflect_v1(channel, MessageRequest::ListServices(String::new())).await;
    let MessageResponse::ListServicesResponse(list) = response else {
        panic!("expected ListServicesResponse, got {response:?}");
    };
    let names: Vec<String> = list.service.into_iter().map(|s| s.name).collect();
    for expected in ["greeter.Greeter", "grpc.reflection.v1.ServerReflection"] {
        assert!(
            names.iter().any(|n| n == expected),
            "expected {expected} in advertised services, got {names:?}"
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[r2e::test]
async fn reflection_lists_services_on_separate_port() {
    let grpc_port = free_port();
    let http_port = free_port();

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port(format!("127.0.0.1:{grpc_port}")).with_reflection())
        .build_state()
        .await
        .register_grpc_service::<TestGreeter>();

    let prepared = app.prepare(&format!("127.0.0.1:{http_port}"));
    let stop = prepared.stop_handle();
    let server = tokio::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });

    let channel = connect_channel(grpc_port).await;
    assert_list_services(channel.clone()).await;

    // Symbol lookup ("grpcurl describe"): the descriptor set declared on the
    // service resolves the fully-qualified service name.
    let response = reflect_v1(
        channel.clone(),
        MessageRequest::FileContainingSymbol("greeter.Greeter".into()),
    )
    .await;
    let MessageResponse::FileDescriptorResponse(files) = response else {
        panic!("expected FileDescriptorResponse, got {response:?}");
    };
    assert!(
        !files.file_descriptor_proto.is_empty(),
        "expected at least one file descriptor for greeter.Greeter"
    );

    // v1alpha is served too (older grpcurl versions speak only v1alpha).
    use tonic_reflection::pb::v1alpha;
    let mut client =
        v1alpha::server_reflection_client::ServerReflectionClient::new(channel);
    let request = v1alpha::ServerReflectionRequest {
        host: String::new(),
        message_request: Some(
            v1alpha::server_reflection_request::MessageRequest::ListServices(String::new()),
        ),
    };
    let mut stream = client
        .server_reflection_info(tokio_stream::iter(vec![request]))
        .await
        .expect("v1alpha ServerReflectionInfo call failed")
        .into_inner();
    let response = stream
        .message()
        .await
        .expect("v1alpha reflection stream errored")
        .expect("v1alpha reflection stream closed without a response")
        .message_response
        .expect("v1alpha reflection response carried no message");
    let v1alpha::server_reflection_response::MessageResponse::ListServicesResponse(list) =
        response
    else {
        panic!("expected v1alpha ListServicesResponse, got {response:?}");
    };
    assert!(
        list.service.iter().any(|s| s.name == "greeter.Greeter"),
        "expected greeter.Greeter in v1alpha services"
    );

    stop_and_await_clean(stop, server).await;
}

#[r2e::test]
async fn reflection_lists_services_multiplexed() {
    let port = free_port();

    let app = AppBuilder::new()
        .plugin(GrpcServer::multiplexed().with_reflection())
        .build_state()
        .await
        .register_grpc_service::<TestGreeter>();

    let prepared = app.prepare(&format!("127.0.0.1:{port}"));
    let stop = prepared.stop_handle();
    let server = tokio::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });

    let channel = connect_channel(port).await;
    assert_list_services(channel).await;

    stop_and_await_clean(stop, server).await;
}
