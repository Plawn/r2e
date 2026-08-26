//! Rate-limit bucket scoping, end to end.
//!
//! Two public controllers each expose a handler named `start`. With a budget of
//! 1 request per 60 s, exhausting one must NOT rate-limit the other: bucket
//! keys are scoped by controller **and** handler name.

use std::net::SocketAddr;
use std::sync::Mutex;

use r2e::config::{ConfigValue, R2eConfig};
use r2e::prelude::*;
use r2e::r2e_rate_limit::{ConfiguredPreRateLimit, PreRateLimit, RateLimitRegistry};
use r2e_test::{TestApp, TestServer};

#[controller(path = "/respond")]
pub struct RespondController;

#[routes]
impl RespondController {
    #[post("/start")]
    #[pre_guard(PreRateLimit::global(1, 60))]
    async fn start(&self) -> Json<&'static str> {
        Json("respond:start")
    }

    #[post("/answer")]
    #[pre_guard(PreRateLimit::global(1, 60))]
    async fn answer(&self) -> Json<&'static str> {
        Json("respond:answer")
    }
}

#[controller(path = "/preview")]
pub struct PreviewController;

#[routes]
impl PreviewController {
    #[post("/start")]
    #[pre_guard(PreRateLimit::global(1, 60))]
    async fn start(&self) -> Json<&'static str> {
        Json("preview:start")
    }
}

/// Budget comes from `rate-limit.public.*` instead of literals.
#[controller(path = "/configured")]
pub struct ConfiguredController;

#[routes]
impl ConfiguredController {
    #[post("/start")]
    #[pre_guard(ConfiguredPreRateLimit::global("rate-limit.public").defaults(1000, 60))]
    async fn start(&self) -> Json<&'static str> {
        Json("configured:start")
    }
}

async fn setup() -> TestApp {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("Rate limit test".into()));
    // Budget for the ConfiguredController route: 1 request / 60 s.
    config.set("rate-limit.public.max", ConfigValue::Integer(1));
    config.set("rate-limit.public.window-secs", ConfigValue::Integer(60));

    TestApp::from_builder(
        AppBuilder::new()
            .override_config(config)
            .load_config::<()>()
            .provide(RateLimitRegistry::default())
            .plugin(ErrorHandling)
            .build_state()
            .await
            .register_controllers::<(RespondController, PreviewController, ConfiguredController)>(),
    )
}

#[r2e::test]
async fn homonymous_handlers_in_two_controllers_have_distinct_buckets() {
    let app = setup().await;

    app.post("/respond/start").send().await.assert_ok();
    app.post("/respond/start").send().await.assert_status(StatusCode::TOO_MANY_REQUESTS);

    // Same handler name, different controller: its own bucket.
    app.post("/preview/start").send().await.assert_ok();
    app.post("/preview/start").send().await.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[r2e::test]
async fn handlers_within_one_controller_have_distinct_buckets() {
    let app = setup().await;

    app.post("/respond/start").send().await.assert_ok();
    app.post("/respond/start").send().await.assert_status(StatusCode::TOO_MANY_REQUESTS);

    app.post("/respond/answer").send().await.assert_ok();
}

#[r2e::test]
async fn configured_budget_comes_from_config() {
    let app = setup().await;

    // Literal defaults say 1000/60; config says 1/60 → config wins.
    app.post("/configured/start").send().await.assert_ok();
    app.post("/configured/start").send().await.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

// ── ConnectInfo → PeerAddr → GuardContext.peer_addr, over real TCP ──────────

/// What the guard saw, per request: `(controller_name, peer_addr)`.
/// Only [`PeerController`] writes here.
static SEEN: Mutex<Vec<(&'static str, Option<SocketAddr>)>> = Mutex::new(Vec::new());

/// Self-contained guard that records what the context exposed, and always
/// allows.
pub struct RecordPeer;
impl SelfBuilt for RecordPeer {}

impl<I: Identity> Guard<I> for RecordPeer {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        SEEN.lock()
            .unwrap()
            .push((ctx.controller_name, ctx.peer_addr));
        std::future::ready(Ok(()))
    }
}

#[controller(path = "/peer")]
pub struct PeerController;

#[routes]
impl PeerController {
    #[get("/probe")]
    #[guard(RecordPeer)]
    async fn probe(&self, peer: PeerAddr) -> Json<String> {
        // The same address the guard saw, reached through the request-scoped
        // extractor.
        Json(match peer.0 {
            Some(addr) => addr.ip().to_string(),
            None => "none".to_string(),
        })
    }
}

/// Minimal HTTP/1.1 GET over a raw socket — example-app has no HTTP client
/// dependency, and the point of the test is the transport, not the client.
async fn raw_get(addr: SocketAddr, path: &str) -> String {
    use r2e::rt::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = r2e::rt::TcpStream::connect(addr).await.expect("connect");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

#[r2e::test]
async fn live_server_propagates_the_peer_address_to_guards() {
    SEEN.lock().unwrap().clear();

    let router = AppBuilder::new()
        .load_config::<()>()
        .provide(RateLimitRegistry::default())
        .plugin(ErrorHandling)
        .build_state()
        .await
        .register_controller::<PeerController>()
        .build();

    let server = TestServer::new(router).await;
    let response = raw_get(server.addr(), "/peer/probe").await;

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
    // The handler echoes the peer IP it was given: loopback, not "none".
    assert!(
        response.ends_with("\"127.0.0.1\""),
        "handler did not see the peer address: {response}"
    );

    // And the guard saw the very same address before the handler ran.
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "guard ran exactly once");
    let (controller, peer) = seen[0];
    let peer = peer.expect("ConnectInfo must reach GuardContext.peer_addr");
    assert!(peer.ip().is_loopback(), "unexpected peer address: {peer}");
    assert_ne!(peer.port(), 0, "the peer's ephemeral port is recorded");

    // Bucket prefixes are module-qualified, so two same-named controllers in
    // different modules cannot collide.
    assert_eq!(
        controller,
        concat!(module_path!(), "::PeerController"),
        "the guard context must carry the fully-qualified controller name"
    );
}
