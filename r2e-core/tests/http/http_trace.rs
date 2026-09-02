//! The `HttpTrace` plugin: one span + one summary event per request, request
//! ids, exclusions, and the builder > file > preset > default precedence.
//!
//! Every test drives a router with a **thread-local** subscriber
//! (`tracing::subscriber::set_default`) on a `current_thread` runtime, so the
//! whole request is polled on the test thread and the capture cannot be
//! interleaved by the other tests of this binary. No global subscriber is
//! installed and no environment variable is touched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::http_trace::{resolve_settings, HttpTrace, HttpTraceConfig};
use r2e_core::builtins::request_id::RequestIdPlugin;
use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::{Response, Router, StatusCode};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use crate::support::raw_get_with;

// ── Capture ────────────────────────────────────────────────────────────────

/// One recorded span or event: its identity plus its fields as strings.
#[derive(Clone, Debug)]
struct Rec {
    name: String,
    target: String,
    level: Level,
    fields: HashMap<String, String>,
}

impl Rec {
    fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    fn mentions(&self, needle: &str) -> bool {
        self.fields.values().any(|v| v.contains(needle))
    }
}

struct FieldRecorder<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Default, Clone)]
struct Capture {
    /// Spans keyed by id, so a late `Span::record` lands on the right one.
    spans: Arc<Mutex<Vec<(u64, Rec)>>>,
    events: Arc<Mutex<Vec<Rec>>>,
}

impl Capture {
    fn spans(&self) -> Vec<Rec> {
        self.spans
            .lock()
            .unwrap()
            .iter()
            .map(|(_, rec)| rec.clone())
            .collect()
    }

    fn events(&self) -> Vec<Rec> {
        self.events.lock().unwrap().clone()
    }

    /// The single `request` span of the request just driven.
    fn request_span(&self) -> Rec {
        let spans = self.spans();
        let mut it = spans.iter().filter(|s| s.name == "request");
        let span = it.next().cloned().expect("a `request` span");
        assert!(it.next().is_none(), "expected exactly one request span");
        span
    }

    /// Summary events (`request completed`) of the request just driven.
    fn summaries(&self) -> Vec<Rec> {
        self.events()
            .into_iter()
            .filter(|e| e.field("message") == Some("request completed"))
            .collect()
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        attrs.record(&mut FieldRecorder(&mut fields));
        self.spans.lock().unwrap().push((
            id.into_u64(),
            Rec {
                name: attrs.metadata().name().to_string(),
                target: attrs.metadata().target().to_string(),
                level: *attrs.metadata().level(),
                fields,
            },
        ));
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, _ctx: Context<'_, S>) {
        // `Span::record` lands here — `request_id`, `status`, … are recorded
        // after creation, so the capture has to follow them.
        let mut spans = self.spans.lock().unwrap();
        if let Some((_, rec)) = spans.iter_mut().find(|(sid, _)| *sid == id.into_u64()) {
            values.record(&mut FieldRecorder(&mut rec.fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldRecorder(&mut fields));
        self.events.lock().unwrap().push(Rec {
            name: event.metadata().name().to_string(),
            target: event.metadata().target().to_string(),
            level: *event.metadata().level(),
            fields,
        });
    }
}

// ── Fixtures ───────────────────────────────────────────────────────────────

fn routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route("/users/{id}", get(|| async { "user" }))
        .route("/health", get(|| async { "ok" }))
        .route(
            "/boom",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "down") }),
        )
}

/// Build the app router with `HttpTrace` configured by `yaml` (the whole
/// application config, so `trace:` lives at the top level).
async fn app(plugin: HttpTrace, yaml: &str) -> Router {
    AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).expect("valid yaml"))
        .load_config::<()>()
        .plugin(plugin)
        .build_state()
        .await
        .merge_router(routes())
        .build()
}

/// Drive one request under a fresh capture and return it with the response.
async fn drive(router: Router, path: &str, headers: &[(&str, &str)]) -> (Capture, Response) {
    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    let response = raw_get_with(router, path, headers).await;
    (capture, response)
}

const NO_CONFIG: &str = "server:\n  port: 0\n";

// ── Span shape ─────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn span_carries_the_route_template_and_never_the_raw_path() {
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, response) = drive(router, "/users/s3cr3t-token", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    let span = capture.request_span();
    assert_eq!(span.target, "r2e::http");
    assert_eq!(span.field("route"), Some("/users/{id}"));
    assert_eq!(span.field("method"), Some("GET"));
    assert!(
        !span.mentions("s3cr3t"),
        "the raw path must not reach the span: {span:?}"
    );
    // …nor the summary event, which is off by default for path/query.
    for event in capture.summaries() {
        assert!(
            !event.mentions("s3cr3t"),
            "raw path on the event: {event:?}"
        );
    }
}

// ── Request id ─────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn a_request_id_is_minted_and_echoed_when_absent() {
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, response) = drive(router, "/users/7", &[]).await;

    let echoed = response
        .headers()
        .get("x-request-id")
        .expect("echoed request id")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!echoed.is_empty());
    assert_eq!(capture.request_span().field("request_id"), Some(&*echoed));
}

#[r2e_core::test(flavor = "current_thread")]
async fn an_inbound_request_id_is_reused_and_echoed() {
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, response) = drive(router, "/users/7", &[("x-request-id", "abc-123")]).await;

    assert_eq!(
        response.headers().get("x-request-id").unwrap(),
        &http_value("abc-123")
    );
    assert_eq!(capture.request_span().field("request_id"), Some("abc-123"));
}

fn http_value(s: &str) -> r2e_core::http::HeaderValue {
    r2e_core::http::HeaderValue::from_str(s).unwrap()
}

#[r2e_core::test(flavor = "current_thread")]
async fn the_request_id_plugin_agrees_in_either_install_order() {
    for outer_first in [true, false] {
        let builder = AppBuilder::new()
            .override_config(R2eConfig::from_yaml_str(NO_CONFIG).unwrap())
            .load_config::<()>();
        let state = if outer_first {
            builder
                .plugin(RequestIdPlugin)
                .plugin(HttpTrace::new())
                .build_state()
                .await
                .merge_router(routes())
                .build()
        } else {
            builder
                .plugin(HttpTrace::new())
                .plugin(RequestIdPlugin)
                .build_state()
                .await
                .merge_router(routes())
                .build()
        };
        let router = state;
        let (capture, response) = drive(router, "/users/7", &[]).await;

        let ids: Vec<_> = response.headers().get_all("x-request-id").iter().collect();
        assert_eq!(ids.len(), 1, "one id per response (order: {outer_first})");
        let echoed = ids[0].to_str().unwrap();
        assert_eq!(
            capture.request_span().field("request_id"),
            Some(echoed),
            "span and response disagree (order: {outer_first})"
        );
    }
}

// ── Exclusions ─────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn excluded_raw_paths_get_no_span_no_event_and_no_request_id() {
    // `/health` is the built-in default exclusion.
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, response) = drive(router, "/health", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get("x-request-id").is_none(),
        "an excluded request is a pure pass-through"
    );
    assert!(capture.spans().iter().all(|s| s.name != "request"));
    assert!(capture.summaries().is_empty());
}

#[r2e_core::test(flavor = "current_thread")]
async fn the_exclusion_prefix_also_matches_the_route_label() {
    // `/users/{` matches the route template but not the raw path `/users/7`.
    let yaml = "trace:\n  exclude-paths: [\"/users/{\"]\n";
    let router = app(HttpTrace::new(), yaml).await;
    let (capture, response) = drive(router, "/users/7", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-request-id").is_none());
    assert!(capture.spans().iter().all(|s| s.name != "request"));
    assert!(capture.summaries().is_empty());
}

// ── Summary event ──────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn a_successful_request_logs_exactly_one_info_summary() {
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, _) = drive(router, "/users/7", &[]).await;

    let summaries = capture.summaries();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].level, Level::INFO);
    assert_eq!(summaries[0].target, "r2e::http");
    assert_eq!(summaries[0].field("status"), Some("200"));
    assert!(summaries[0].fields.contains_key("latency_ms"));
    assert!(summaries[0].field("path").is_none(), "path is opt-in");
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_server_error_logs_exactly_one_error_summary() {
    let router = app(HttpTrace::new(), NO_CONFIG).await;
    let (capture, response) = drive(router, "/boom", &[]).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let summaries = capture.summaries();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].level, Level::ERROR);
    assert_eq!(summaries[0].field("status"), Some("503"));
}

#[r2e_core::test(flavor = "current_thread")]
async fn record_path_puts_the_path_on_the_event_only() {
    let yaml = "trace:\n  record-path: true\n  record-query: true\n";
    let router = app(HttpTrace::new(), yaml).await;
    let (capture, _) = drive(router, "/users/7?debug=1", &[]).await;

    let summaries = capture.summaries();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].field("path"), Some("/users/7"));
    assert_eq!(summaries[0].field("query"), Some("debug=1"));

    let span = capture.request_span();
    assert!(
        !span.mentions("/users/7") && !span.mentions("debug=1"),
        "the raw path/query must stay off the span: {span:?}"
    );
}

// ── Enrichment channel ─────────────────────────────────────────────────────

use r2e_core::http::Extension;
use r2e_core::{MakeRequestSpan, RequestOutcome, RequestSpan, SpanState};
use r2e_core::web::request_head::RequestHead;

use crate::support::body_string;

/// The app-side per-request state: one fact written by the handler, read back
/// by `on_response`.
#[derive(Default)]
struct Facts(Mutex<Option<String>>);

/// A span shape declaring a domain field (`session_id`) plus a state slot.
struct EnrichedSpan;

impl MakeRequestSpan for EnrichedSpan {
    fn make_span(
        &self,
        _req: &RequestHead<'_>,
        route: &str,
        _request_id: Option<&str>,
    ) -> tracing::Span {
        tracing::info_span!(
            target: "r2e::http",
            "request",
            route,
            session_id = tracing::field::Empty,
            status = tracing::field::Empty,
        )
    }

    fn make_state(&self, _req: &RequestHead<'_>) -> Option<SpanState> {
        Some(SpanState::new(Facts::default()))
    }

    fn on_response(
        &self,
        span: &tracing::Span,
        outcome: &RequestOutcome<'_>,
        state: Option<&SpanState>,
    ) {
        let session = state
            .and_then(|s| s.get::<Facts>())
            .and_then(|f| f.0.lock().unwrap().clone());
        let _enter = span.enter();
        tracing::info!(
            target: "r2e::http",
            status = outcome.status.map(|s| s.as_u16()),
            session_id = session.as_deref(),
            "request completed"
        );
    }
}

fn enrichment_routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route(
            "/session",
            get(
                |span: RequestSpan, Extension(state): Extension<SpanState>| async move {
                    span.record("session_id", "sess-42");
                    if let Some(facts) = state.get::<Facts>() {
                        *facts.0.lock().unwrap() = Some("sess-42".into());
                    }
                    "ok"
                },
            ),
        )
        // Reports whether the request carried a live `RequestSpan` extension —
        // the extractor is infallible and falls back to `Span::none()`.
        .route(
            "/probe",
            get(|span: RequestSpan| async move {
                if span.span().is_none() {
                    "no-span"
                } else {
                    "span"
                }
            }),
        )
        .route(
            "/health",
            get(|span: RequestSpan| async move {
                if span.span().is_none() {
                    "no-span"
                } else {
                    "span"
                }
            }),
        )
}

async fn enrichment_app(plugin: HttpTrace) -> Router {
    AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(NO_CONFIG).expect("valid yaml"))
        .load_config::<()>()
        .plugin(plugin)
        .build_state()
        .await
        .merge_router(enrichment_routes())
        .build()
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_handler_records_declared_fields_through_the_request_span_extension() {
    let plugin = HttpTrace::builder().make_span(EnrichedSpan).build();
    let router = enrichment_app(plugin).await;
    let (capture, response) = drive(router, "/session", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        capture.request_span().field("session_id"),
        Some("sess-42"),
        "the handler's record must land on the request span"
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn make_state_roundtrips_from_the_handler_to_on_response() {
    let plugin = HttpTrace::builder().make_span(EnrichedSpan).build();
    let router = enrichment_app(plugin).await;
    let (capture, _) = drive(router, "/session", &[]).await;

    let summaries = capture.summaries();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(
        summaries[0].field("session_id"),
        Some("sess-42"),
        "the fact written by the handler must reach the custom summary event"
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn the_default_span_shape_also_publishes_the_request_span_extension() {
    let router = enrichment_app(HttpTrace::new()).await;
    let (_, response) = drive(router, "/probe", &[]).await;

    assert_eq!(body_string(response).await, "span");
}

#[r2e_core::test(flavor = "current_thread")]
async fn an_excluded_request_publishes_no_request_span_extension() {
    // `/health` is excluded by default: the extractor falls back to a no-op
    // `Span::none()` instead of failing.
    let router = enrichment_app(HttpTrace::new()).await;
    let (_, response) = drive(router, "/health", &[]).await;

    assert_eq!(body_string(response).await, "no-span");
}

// ── Gate + precedence ──────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn trace_enabled_false_installs_no_layer() {
    let yaml = "trace:\n  enabled: false\n";
    let router = app(HttpTrace::new(), yaml).await;
    let (capture, response) = drive(router, "/users/7", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-request-id").is_none());
    assert!(capture.spans().iter().all(|s| s.name != "request"));
    assert!(capture.summaries().is_empty());
}

#[r2e_core::test(flavor = "current_thread")]
async fn an_explicit_builder_exclusion_beats_the_file() {
    // The file excludes `/users`, the builder does not — the builder wins, so
    // the request is traced.
    let yaml = "trace:\n  exclude-paths: [\"/users\"]\n";
    let plugin = HttpTrace::builder().exclude_path("/nothing").build();
    let router = app(plugin, yaml).await;
    let (capture, response) = drive(router, "/users/7", &[]).await;

    assert!(response.headers().get("x-request-id").is_some());
    assert_eq!(capture.request_span().field("route"), Some("/users/{id}"));
}

#[test]
fn precedence_is_builder_then_file_then_preset_then_default() {
    let paths = |settings: &r2e_core::HttpTraceSettings| settings.exclude_paths.to_vec();

    let builder = HttpTraceConfig {
        exclude_paths: Some(vec!["/from-builder".into()]),
        ..HttpTraceConfig::default()
    };
    let file = HttpTraceConfig {
        exclude_paths: Some(vec!["/from-file".into()]),
        ..HttpTraceConfig::default()
    };
    let preset = HttpTraceConfig {
        exclude_paths: Some(vec!["/from-preset".into()]),
        ..HttpTraceConfig::default()
    };

    let all = resolve_settings(builder, Some(file.clone()), Some(preset.clone())).unwrap();
    assert_eq!(paths(&all), vec!["/from-builder".to_string()]);

    let file_wins =
        resolve_settings(HttpTraceConfig::default(), Some(file), Some(preset.clone())).unwrap();
    assert_eq!(paths(&file_wins), vec!["/from-file".to_string()]);

    let preset_wins = resolve_settings(HttpTraceConfig::default(), None, Some(preset)).unwrap();
    assert_eq!(paths(&preset_wins), vec!["/from-preset".to_string()]);

    let defaults = resolve_settings(HttpTraceConfig::default(), None, None).unwrap();
    assert_eq!(
        paths(&defaults),
        vec!["/health".to_string(), "/metrics".to_string()]
    );
}

#[test]
fn an_invalid_capture_header_is_a_boot_error() {
    let builder = HttpTraceConfig {
        capture_headers: Some(vec!["not a header".into()]),
        ..HttpTraceConfig::default()
    };
    let err = resolve_settings(builder, None, None).expect_err("invalid header name");
    assert!(
        err.to_string().contains("capture-headers"),
        "unhelpful error: {err}"
    );
}
