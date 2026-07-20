//! Span-field cardinality of the OTel trace middleware.
//!
//! `http.route` must stay bounded under arbitrary-path traffic: matched
//! requests carry the route template (`/users/{id}`), unmatched requests
//! collapse into the `UNMATCHED_PATH_LABEL` sentinel, and non-standard
//! methods collapse into `OTHER_METHOD_LABEL`. The raw path is recorded as
//! `url.path`, an unbounded per-span attribute.

use r2e_core::http::labels::{OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL};
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router};
use r2e_observability::middleware::OtelTraceLayer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

/// Records the fields of every span named "HTTP request" created while active.
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
        if attrs.metadata().name() != "HTTP request" {
            return;
        }
        let mut fields = HashMap::new();
        attrs.record(&mut FieldRecorder(&mut fields));
        self.spans.lock().unwrap().push(fields);
    }
}

/// Send one request through a router wrapped in `OtelTraceLayer` and return
/// the fields of the request span it created.
async fn request_span_fields(method: &str, path: &str) -> HashMap<String, String> {
    let capture = SpanCapture::default();
    let subscriber = Registry::default().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let router = Router::new()
        .route("/users/{id}", get(|| async { "user" }))
        .layer(OtelTraceLayer::new(vec![]));
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
    assert_eq!(fields["url.path"], "/users/7");
    assert_eq!(fields["http.method"], "GET");
}

#[tokio::test]
async fn unmatched_requests_collapse_into_the_sentinel_route() {
    let junk = "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php";
    let fields = request_span_fields("GET", junk).await;
    assert_eq!(fields["http.route"], UNMATCHED_PATH_LABEL);
    // The raw path stays available as a per-span attribute.
    assert_eq!(fields["url.path"], junk);
}

#[tokio::test]
async fn extension_methods_collapse_into_the_other_label() {
    let fields = request_span_fields("PURGE", "/users/7").await;
    assert_eq!(fields["http.method"], OTHER_METHOD_LABEL);
}
