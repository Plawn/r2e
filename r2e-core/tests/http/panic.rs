//! The catch-panic layer: what an operator gets when a handler panics.
//!
//! Pins the four things ticket #1017 asked for — a JSON 500, exactly one
//! structured `error` line carrying the request's `request_id`, the panic
//! message on that line, and the application hook fired once — plus the
//! reason the layer moved inside `HttpTrace`: the panicking request must
//! still produce a summary line and a recorded 500 status.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::http_trace::HttpTrace;
use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::{Response, Router, StatusCode};
use r2e_core::PanicHook;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

use crate::fixtures::{Capture, Rec};
use crate::support::{body_string, raw_get_with};

const NO_CONFIG: &str = "server:\n  port: 0\n";

/// `PANIC_TARGET` — spelled out so a rename of the constant is a test failure,
/// not a silent break of every log pipeline filtering on it.
const PANIC_TARGET: &str = "r2e::panic";

fn routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route(
            "/panic/{id}",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                ""
            }),
        )
        .route(
            "/panic-string",
            get(|| async {
                panic!("{}", String::from("owned boom"));
                #[allow(unreachable_code)]
                ""
            }),
        )
        .route("/ok", get(|| async { "ok" }))
}

/// `(message, route)` of every hook invocation, in order.
type Reports = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// The panic events of the request just driven.
fn panics(capture: &Capture) -> Vec<Rec> {
    capture
        .events()
        .into_iter()
        .filter(|e| e.target == PANIC_TARGET)
        .collect()
}

/// Build a traced app, optionally with a panic hook, and drive one GET under
/// a fresh thread-local capture.
async fn drive(path: &str, hook: Option<PanicHook>) -> (Capture, Response) {
    let builder = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(NO_CONFIG).expect("valid yaml"))
        .load_config::<()>()
        .plugin(HttpTrace::new());
    let builder = match hook {
        Some(hook) => builder.on_panic(move |report| hook(report)),
        None => builder,
    };
    let router = builder.build_state().await.merge_router(routes()).build();

    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    let response = raw_get_with(router, path, &[]).await;
    (capture, response)
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_panicking_handler_answers_the_json_500_contract() {
    let (_capture, response) = drive("/panic/7", None).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response
            .headers()
            .get(r2e_core::http::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        body_string(response).await,
        r#"{"error":"Internal server error"}"#
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn one_error_line_carries_the_request_id_the_message_and_the_route() {
    let (capture, response) = drive("/panic/7", None).await;

    let events = panics(&capture);
    assert_eq!(
        events.len(),
        1,
        "expected exactly one panic event: {events:?}"
    );
    let event = &events[0];
    assert_eq!(event.level, Level::ERROR);
    assert_eq!(event.field("panic_message"), Some("boom"));
    assert_eq!(event.field("route"), Some("/panic/{id}"));

    // The correlation the ticket is about: the line is emitted inside the
    // request span, so it inherits the id echoed back to the client.
    let echoed = response
        .headers()
        .get("x-request-id")
        .expect("echoed request id")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(capture.request_span().field("request_id"), Some(&*echoed));
}

#[r2e_core::test(flavor = "current_thread")]
async fn an_owned_string_payload_is_downcast_too() {
    let (capture, _response) = drive("/panic-string", None).await;

    let events = panics(&capture);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].field("panic_message"), Some("owned boom"));
}

#[r2e_core::test(flavor = "current_thread")]
async fn the_application_hook_fires_exactly_once_with_the_message_and_route() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen: Reports = Arc::new(Mutex::new(Vec::new()));

    let (calls_h, seen_h) = (Arc::clone(&calls), Arc::clone(&seen));
    let hook: PanicHook = Arc::new(move |report| {
        calls_h.fetch_add(1, Ordering::SeqCst);
        seen_h.lock().unwrap().push((
            report.message().to_owned(),
            report.route().map(str::to_owned),
        ));
    });

    let (_capture, response) = drive("/panic/7", Some(hook)).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[("boom".to_owned(), Some("/panic/{id}".to_owned()))]
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_healthy_request_fires_neither_the_hook_nor_a_panic_event() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_h = Arc::clone(&calls);
    let hook: Arc<dyn Fn(&r2e_core::runtime::panic::PanicReport<'_>) + Send + Sync> =
        Arc::new(move |_| {
            calls_h.fetch_add(1, Ordering::SeqCst);
        });

    let (capture, response) = drive("/ok", Some(hook)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(panics(&capture).is_empty());
}

/// The hook is observability: its own panic must neither change the response
/// nor unwind into the outermost net as a second, routeless panic.
#[r2e_core::test(flavor = "current_thread")]
async fn a_panicking_hook_is_contained_and_the_500_is_unchanged() {
    let hook: PanicHook = Arc::new(|_| panic!("hook exploded"));

    let (capture, response) = drive("/panic/7", Some(hook)).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        body_string(response).await,
        r#"{"error":"Internal server error"}"#
    );
    let events = panics(&capture);
    assert_eq!(events.len(), 2, "handler line + hook line: {events:?}");
    assert_eq!(events[0].field("panic_message"), Some("boom"));
    assert_eq!(events[1].field("panic_message"), Some("hook exploded"));
    assert_eq!(events[1].field("route"), Some("/panic/{id}"));
    assert_eq!(
        capture.summaries().len(),
        1,
        "no second panic reached the outer net"
    );
}

/// The whole reason the layer moved *below* `HttpTrace`: the unwind used to
/// cross it, so a panicking request produced no summary line and no recorded
/// status — nothing to build a 5xx alert on.
#[r2e_core::test(flavor = "current_thread")]
async fn the_panic_still_travels_the_instrumented_response_path() {
    let (capture, response) = drive("/panic/7", None).await;

    assert!(response.headers().contains_key("x-request-id"));
    assert_eq!(capture.request_span().field("status"), Some("500"));

    let summaries = capture.summaries();
    assert_eq!(
        summaries.len(),
        1,
        "expected one summary line: {summaries:?}"
    );
    assert_eq!(summaries[0].level, Level::ERROR);
    assert_eq!(summaries[0].field("status"), Some("500"));
}
