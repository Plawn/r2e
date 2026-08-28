//! e2e: `GrpcServer::multiplexed().with_grpc_web()` serves grpc-web on the
//! HTTP port — binary and `-text`, over HTTP/1.1 and HTTP/2 — with the
//! grpc-web trailer frame a browser client needs, while native gRPC and
//! plain HTTP on the same port are unchanged. Without `with_grpc_web()` the
//! multiplexer keeps answering 415.

mod common;

use base64::Engine;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use common::{connect_channel, free_port, stop_and_await_clean};
use http_body_util::BodyExt;
use prost::Message;
use r2e::prelude::*;
use r2e::r2e_grpc::{AppBuilderGrpcExt, GrpcServer};
use r2e::rt::io::{AsyncReadExt, AsyncWriteExt};

pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::greeter_client::GreeterClient;
use proto::greeter::{HelloReply, HelloRequest};

const SAY_HELLO: &str = "/greeter.Greeter/SayHello";

#[controller]
pub struct WebGreeter {}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl WebGreeter {
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
        _request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Err(tonic::Status::permission_denied("nope"))
    }
}

#[controller(path = "/api")]
pub struct PingController {}

#[routes]
impl PingController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

// ── grpc-web framing helpers ────────────────────────────────────────────

/// One length-prefixed grpc(-web) frame: `flag(1) | len(4, BE) | payload`.
fn frame(flag: u8, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(5 + payload.len());
    b.put_u8(flag);
    b.put_u32(payload.len() as u32);
    b.put_slice(payload);
    b.freeze()
}

fn hello_request(name: &str) -> Bytes {
    frame(0, &HelloRequest { name: name.into() }.encode_to_vec())
}

/// Split a grpc-web response body into `(flag, payload)` frames.
fn frames(mut body: Bytes) -> Vec<(u8, Bytes)> {
    let mut out = Vec::new();
    while body.has_remaining() {
        assert!(body.remaining() >= 5, "truncated frame header: {body:?}");
        let flag = body.get_u8();
        let len = body.get_u32() as usize;
        assert!(body.remaining() >= len, "truncated frame payload");
        out.push((flag, body.split_to(len)));
    }
    out
}

/// A valid grpc-web response = exactly one message frame decoding to the
/// reply, followed by a trailer frame (flag `0x80`) carrying `grpc-status: 0`
/// — the frame the raw tonic services never produced (ticket #965).
fn assert_grpc_web_reply(body: Bytes, expected_message: &str) {
    let frames = frames(body);
    assert_eq!(
        frames.len(),
        2,
        "expected message + trailer frame: {frames:?}"
    );
    assert_eq!(frames[0].0, 0, "first frame is the message");
    let reply = HelloReply::decode(frames[0].1.clone()).unwrap();
    assert_eq!(reply.message, expected_message);
    assert_eq!(frames[1].0 & 0x80, 0x80, "second frame is a trailer frame");
    let trailers = String::from_utf8_lossy(&frames[1].1).to_ascii_lowercase();
    assert!(
        trailers.contains("grpc-status:0") || trailers.contains("grpc-status: 0"),
        "trailer frame should carry grpc-status 0: {trailers:?}"
    );
}

/// A `grpc-web-text` body is a concatenation of base64 blocks (one per
/// frame, each padded); decode block by block.
fn decode_grpc_web_text(body: &[u8]) -> Bytes {
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = std::str::from_utf8(body).expect("-text body is ASCII base64");
    let mut out = BytesMut::new();
    let mut block = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        block.push(c);
        if c == '=' && chars.peek() != Some(&'=') {
            out.put_slice(&b64.decode(&block).expect("-text block is base64"));
            block.clear();
        }
    }
    if !block.is_empty() {
        out.put_slice(&b64.decode(&block).expect("-text tail is base64"));
    }
    out.freeze()
}

// ── boot ────────────────────────────────────────────────────────────────

struct Server {
    port: u16,
    stop: StopHandle,
    task: r2e::rt::JobHandle<Result<(), String>>,
}

async fn boot(server: GrpcServer) -> Server {
    let port = free_port();
    let app = AppBuilder::new()
        .plugin(server)
        .build_state()
        .await
        .register_grpc_service::<WebGreeter>()
        .register_controller::<PingController>();
    let prepared = app.prepare(&format!("127.0.0.1:{port}"));
    let stop = prepared.stop_handle();
    let task = r2e::rt::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });
    // Wait for the listener (also proves native gRPC still works).
    let channel = connect_channel(port).await;
    drop(channel);
    Server { port, stop, task }
}

/// Raw HTTP/1.1 exchange; returns `(status line, headers, body)`.
async fn h1(port: u16, request: Vec<u8>) -> (String, String, Bytes) {
    let mut stream = r2e::rt::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream.write_all(&request).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no header/body separator");
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let (status, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    let mut body = Bytes::copy_from_slice(&buf[split + 4..]);
    // `Connection: close` responses may still be chunked (streaming bodies).
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = dechunk(body);
    }
    (status.to_string(), headers.to_ascii_lowercase(), body)
}

fn dechunk(mut body: Bytes) -> Bytes {
    let mut out = BytesMut::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("chunk size line");
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..line_end]).unwrap().trim(), 16)
                .unwrap();
        body.advance(line_end + 2);
        if size == 0 {
            break;
        }
        out.put_slice(&body[..size]);
        body.advance(size + 2);
    }
    out.freeze()
}

/// grpc-web clients send `Accept` = the content-type they speak; `tonic-web`
/// picks the response encoding (binary vs base64 `-text`) from it.
fn h1_post(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut req = format!(
        "POST {SAY_HELLO} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\n\
         Accept: {content_type}\r\nX-Grpc-Web: 1\r\nOrigin: http://app.example\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(body);
    req
}

// ── tests ───────────────────────────────────────────────────────────────

#[r2e::test]
async fn grpc_web_binary_over_http1() {
    let srv = boot(GrpcServer::multiplexed().with_grpc_web()).await;

    let (status, headers, body) = h1(
        srv.port,
        h1_post("application/grpc-web+proto", &hello_request("h1")),
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(
        headers.contains("content-type: application/grpc-web+proto"),
        "{headers}"
    );
    // CORS: the default policy lets the browser read the response.
    assert!(
        headers.contains("access-control-allow-origin: *"),
        "{headers}"
    );
    assert!(
        headers.contains("grpc-status"),
        "grpc-status exposed: {headers}"
    );
    assert_grpc_web_reply(body, "hi h1");

    stop_and_await_clean(srv.stop, srv.task).await;
}

#[r2e::test]
async fn grpc_web_text_is_base64_over_http1() {
    let srv = boot(GrpcServer::multiplexed().with_grpc_web()).await;

    let b64 = base64::engine::general_purpose::STANDARD;
    let encoded = b64.encode(hello_request("text"));
    let (status, headers, body) = h1(
        srv.port,
        h1_post("application/grpc-web-text+proto", encoded.as_bytes()),
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(
        headers.contains("content-type: application/grpc-web-text+proto"),
        "{headers}"
    );
    assert_grpc_web_reply(decode_grpc_web_text(&body), "hi text");

    stop_and_await_clean(srv.stop, srv.task).await;
}

#[r2e::test]
async fn grpc_web_preflight_is_answered_by_the_grpc_web_arm() {
    // No `Cors` plugin installed: the preflight must still succeed.
    let srv = boot(GrpcServer::multiplexed().with_grpc_web()).await;

    let req = format!(
        "OPTIONS {SAY_HELLO} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://app.example\r\n\
         Access-Control-Request-Method: POST\r\n\
         Access-Control-Request-Headers: content-type,x-grpc-web,x-user-agent\r\n\
         Connection: close\r\n\r\n"
    );
    let (status, headers, _) = h1(srv.port, req.into_bytes()).await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(
        headers.contains("access-control-allow-origin: *"),
        "{headers}"
    );
    assert!(
        headers.contains("access-control-allow-methods"),
        "{headers}"
    );
    assert!(
        headers.contains("x-grpc-web"),
        "request headers mirrored: {headers}"
    );

    stop_and_await_clean(srv.stop, srv.task).await;
}

#[r2e::test]
async fn grpc_web_binary_over_http2() {
    let srv = boot(GrpcServer::multiplexed().with_grpc_web()).await;

    let client: hyper_util::client::legacy::Client<_, http_body_util::Full<Bytes>> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .http2_only(true)
            .build_http();
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!("http://127.0.0.1:{}{SAY_HELLO}", srv.port))
        .header("content-type", "application/grpc-web+proto")
        .header("accept", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(http_body_util::Full::new(hello_request("h2")))
        .unwrap();
    let resp = client.request(req).await.unwrap();
    assert_eq!(resp.version(), http::Version::HTTP_2);
    assert_eq!(resp.status(), http::StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/grpc-web+proto"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_grpc_web_reply(body, "hi h2");

    stop_and_await_clean(srv.stop, srv.task).await;
}

#[r2e::test]
async fn native_grpc_and_http_are_unchanged_with_grpc_web_enabled() {
    let srv = boot(GrpcServer::multiplexed().with_grpc_web()).await;

    let mut client = GreeterClient::new(connect_channel(srv.port).await);
    let resp = client
        .say_hello(HelloRequest {
            name: "native".into(),
        })
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hi native");

    let (status, _, body) = h1(
        srv.port,
        b"GET /api/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".to_vec(),
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(&body[..], b"pong");

    stop_and_await_clean(srv.stop, srv.task).await;
}

#[r2e::test]
async fn without_with_grpc_web_the_multiplexer_still_answers_415() {
    let srv = boot(GrpcServer::multiplexed()).await;

    let (status, _, _) = h1(
        srv.port,
        h1_post("application/grpc-web+proto", &hello_request("no")),
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 415"), "{status}");

    // And a grpc-web preflight is left to the HTTP router (no Cors plugin
    // here → the router's own answer, not a CORS grant).
    let req = format!(
        "OPTIONS {SAY_HELLO} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://app.example\r\n\
         Access-Control-Request-Method: POST\r\n\
         Access-Control-Request-Headers: content-type,x-grpc-web\r\n\
         Connection: close\r\n\r\n"
    );
    let (_, headers, _) = h1(srv.port, req.into_bytes()).await;
    assert!(
        !headers.contains("access-control-allow-origin"),
        "{headers}"
    );

    stop_and_await_clean(srv.stop, srv.task).await;
}
