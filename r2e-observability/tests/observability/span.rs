//! Span-field cardinality and propagation of the OTel span shape installed by
//! `Observability` on `r2e-core`'s shared `HttpTraceLayer`.
//!
//! `http.route` must stay bounded under arbitrary-path traffic: matched
//! requests carry the route template (`/users/{id}`), unmatched requests
//! collapse into the `UNMATCHED_PATH_LABEL` sentinel, and non-standard methods
//! collapse into `OTHER_METHOD_LABEL`. The raw path is never a span field — it
//! reaches the summary event only under `trace.record-path`.

use http_body_util::BodyExt;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use r2e_core::http::labels::{OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL};
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router};
use r2e_core::{HttpTraceLayer, HttpTraceSettings};
use r2e_observability::{inject_current_context, OtelRequestSpan};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

/// Records the fields of every span named "request" created while active.
#[derive(Default, Clone)]
struct SpanCapture {
    spans: Arc<Mutex<Vec<HashMap<String, String>>>>,
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

impl<S: Subscriber> Layer<S> for SpanCapture {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if attrs.metadata().name() != "request" {
            return;
        }
        let mut fields = HashMap::new();
        attrs.record(&mut FieldRecorder(&mut fields));
        self.spans.lock().unwrap().push(fields);
    }
}

/// Send one request through a router wrapped in the OTel-shaped
/// `HttpTraceLayer` and return the fields of the request span it created.
/// The layer exactly as `Observability` installs it: core's `HttpTraceLayer`
/// with the OTel span shape.
fn otel_layer() -> HttpTraceLayer<OtelRequestSpan> {
    HttpTraceLayer::with_make_span(
        HttpTraceSettings::default(),
        OtelRequestSpan::new(Arc::from(Vec::new())),
    )
}

async fn request_span_fields(method: &str, path: &str) -> HashMap<String, String> {
    let capture = SpanCapture::default();
    let subscriber = Registry::default().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let router = Router::new()
        .route("/users/{id}", get(|| async { "user" }))
        .layer(otel_layer());
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap();

    let spans = capture.spans.lock().unwrap();
    assert_eq!(spans.len(), 1, "expected exactly one request span");
    spans[0].clone()
}

#[tokio::test]
async fn matched_requests_record_the_route_template() {
    let fields = request_span_fields("GET", "/users/7").await;
    assert_eq!(fields["http.route"], "/users/{id}");
    assert_eq!(fields["http.request.method"], "GET");
    // The raw path is deliberately absent from the span: it belongs on the
    // summary event, and only under `trace.record-path`.
    assert!(!fields.contains_key("url.path"), "fields: {fields:?}");
}

#[tokio::test]
async fn unmatched_requests_collapse_into_the_sentinel_route() {
    let junk = "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php";
    let fields = request_span_fields("GET", junk).await;
    assert_eq!(fields["http.route"], UNMATCHED_PATH_LABEL);
    // Not even the unmatched raw path leaks onto the span.
    assert!(
        !fields.values().any(|v| v.contains("phpunit")),
        "fields: {fields:?}"
    );
}

#[tokio::test]
async fn extension_methods_collapse_into_the_other_label() {
    let fields = request_span_fields("PURGE", "/users/7").await;
    assert_eq!(fields["http.request.method"], OTHER_METHOD_LABEL);
}

#[tokio::test(flavor = "current_thread")]
async fn incoming_traceparent_is_used_for_downstream_propagation() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("middleware-test");
    let subscriber = Registry::default().with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let router = Router::new()
        .route(
            "/propagate",
            get(|| async {
                let mut headers = http::HeaderMap::new();
                inject_current_context(&mut headers);
                headers["traceparent"].to_str().unwrap().to_string()
            }),
        )
        .layer(otel_layer());
    let incoming_trace_id = "11111111111111111111111111111111";
    let request = Request::builder()
        .uri("/propagate")
        .header(
            "traceparent",
            format!("00-{incoming_trace_id}-2222222222222222-01"),
        )
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let propagated = std::str::from_utf8(&body).unwrap();
    assert_eq!(propagated.split('-').nth(1), Some(incoming_trace_id));

    drop(subscriber_guard);
    provider.shutdown().unwrap();
}
