//! End-to-end test of the EventBus↔SSE bridge wired in the blueprint:
//! `POST /users` → `UserService` emits `UserCreatedEvent` on the
//! `LocalEventBus` → `bridge_sse` forwards it to `SseTopic<UserCreatedEvent>`
//! → SSE subscribers observe it (served at `/sse/users`).
//!
//! The stream is consumed via the topic bean (the broadcaster-backed HTTP
//! stream is infinite, so the in-process client would hang collecting the
//! body; the `#[sse]` endpoint framing itself is covered by `sse_test.rs`).

use std::pin::Pin;

use example_app::models::UserCreatedEvent;
use r2e::http::response::SseEvent;
use r2e::prelude::*;
use r2e::sse::SseSubscription;
use r2e_test::TestApp;

async fn next_event(sub: &mut SseSubscription) -> Option<SseEvent> {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        std::future::poll_fn(|cx| {
            use futures_core::Stream;
            Pin::new(&mut *sub).poll_next(cx)
        })
        .await
    })
    .await
    .ok()
    .flatten()
    .map(|r| r.unwrap())
}

#[r2e::test(app = example_app::app)]
async fn user_creation_fans_out_to_the_sse_topic(
    app: TestApp,
    #[inject] topic: SseTopic<UserCreatedEvent>,
) {
    // Subscribe before triggering the event, as an SSE client would.
    let mut sub = topic.subscribe();

    app.post("/users")
        .as_user("user-1", &["user"])
        .json(&serde_json::json!({ "name": "Diana", "email": "diana@example.com" }))
        .send()
        .await
        .assert_ok();

    let event = next_event(&mut sub)
        .await
        .expect("the bridge should forward UserCreatedEvent to the SSE topic");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("user_created"),
        "SSE event name should be the topic's: {debug}"
    );
    assert!(
        debug.contains("Diana"),
        "SSE data should carry the serialized event: {debug}"
    );
}

#[r2e::test(app = example_app::app)]
async fn sse_topic_endpoint_is_reachable(app: TestApp) {
    // The infinite stream can't be collected in-process; hit the endpoint
    // over a live server and assert the SSE handshake headers.
    let server = app.serve().await;
    let mut stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(
        &mut stream,
        b"GET /sse/users HTTP/1.1\r\nHost: test\r\nAccept: text/event-stream\r\n\r\n",
    )
    .await
    .unwrap();

    let mut buf = [0u8; 1024];
    let n = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
    )
    .await
    .expect("server should answer the SSE handshake")
    .unwrap();
    let head = String::from_utf8_lossy(&buf[..n]);
    assert!(head.contains("200"), "expected 200, got: {head}");
    assert!(
        head.contains("text/event-stream"),
        "expected SSE content type, got: {head}"
    );
}
