use std::convert::Infallible;

use r2e::config::R2eConfig;
use r2e::http::response::SseEvent;
use r2e::prelude::*;
use r2e_test::TestApp;

// ─── State ───

#[derive(Clone)]
struct SseTestState {
    config: R2eConfig,
}

impl r2e::http::extract::FromRef<SseTestState> for R2eConfig {
    fn from_ref(state: &SseTestState) -> Self {
        state.config.clone()
    }
}

// ─── A simple finite stream for testability ───
// (The broadcaster-backed stream is infinite, so TestApp.send() would hang.)

struct FiniteStream<T> {
    items: std::vec::IntoIter<T>,
}

impl<T: Unpin> futures_core::Stream for FiniteStream<T> {
    type Item = T;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.items.next())
    }
}

fn finite_stream<T: Unpin>(items: Vec<T>) -> FiniteStream<T> {
    FiniteStream {
        items: items.into_iter(),
    }
}

// ─── SSE Controller with finite stream ───

#[derive(Controller)]
#[controller(path = "/sse", state = SseTestState)]
pub struct SseTestController;

#[routes]
impl SseTestController {
    /// Returns a finite stream of 3 events then completes.
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        finite_stream(vec![
            Ok(SseEvent::default().data("hello")),
            Ok(SseEvent::default().event("update").data(r#"{"count":42}"#)),
            Ok(SseEvent::default().data("goodbye")),
        ])
    }
}

async fn setup() -> TestApp {
    let config = R2eConfig::empty();

    let state = SseTestState {
        config: config.clone(),
    };

    TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
            .with_config(config)
            .with(ErrorHandling)
            .register_controller::<SseTestController>(),
    )
}

// ─── Tests ───

#[tokio::test]
async fn test_sse_content_type() {
    let app = setup().await;
    let resp = app.get("/sse/events").send().await.assert_ok();
    let ct = resp.header("content-type").expect("missing content-type");
    assert!(
        ct.contains("text/event-stream"),
        "Expected text/event-stream, got: {}",
        ct
    );
}

#[tokio::test]
async fn test_sse_event_format() {
    let app = setup().await;
    let resp = app.get("/sse/events").send().await.assert_ok();
    let body = resp.text();

    assert!(
        body.contains("data: hello"),
        "SSE body should contain 'data: hello', got: {}",
        body
    );
}

#[tokio::test]
async fn test_sse_typed_event() {
    let app = setup().await;
    let resp = app.get("/sse/events").send().await.assert_ok();
    let body = resp.text();

    assert!(
        body.contains("event: update"),
        "SSE body should contain event type, got: {}",
        body
    );
    assert!(
        body.contains(r#"data: {"count":42}"#),
        "SSE body should contain data payload, got: {}",
        body
    );
}

#[tokio::test]
async fn test_sse_stream_completes() {
    let app = setup().await;
    let resp = app.get("/sse/events").send().await.assert_ok();
    let body = resp.text();

    // All three events should be present
    assert!(body.contains("data: hello"), "missing hello event");
    assert!(body.contains("data: goodbye"), "missing goodbye event");
    assert!(body.contains("event: update"), "missing update event");
}
