//! The OpenTelemetry span shape for the shared `HttpTrace` layer.
//!
//! `Observability` does not own an HTTP middleware of its own any more: it
//! installs `r2e_core`'s [`HttpTraceLayer`](r2e_core::HttpTraceLayer) with the
//! [`OtelRequestSpan`] shape below. Exclusions, request-id handling, timing and
//! the summary event stay in one place, and every request gets **one** span
//! instead of the historical tower-http span + OTel span pair.

use std::sync::Arc;

use opentelemetry::propagation::Extractor;
use r2e_core::http::labels::method_label;
use r2e_core::http::{HeaderMap, HeaderName};
use r2e_core::web::request_head::RequestHead;
use r2e_core::{MakeRequestSpan, RequestOutcome};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Header extractor for OpenTelemetry propagation.
struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// The OpenTelemetry span shape: semantic-convention field names, `otel.kind =
/// "server"`, and the parent context extracted from the inbound `traceparent`.
///
/// The header allow-list is immutable for the app's lifetime and shared behind
/// an [`Arc`]: the HTTP backend clones the wrapped service once per request.
///
/// Unlike the `fmt`-oriented
/// [`DefaultRequestSpan`](r2e_core::DefaultRequestSpan), this one *can* give
/// each captured header its own key — OTel attributes are a runtime map, while
/// `tracing` span fields come from a static callsite.
#[derive(Clone, Debug, Default)]
pub struct OtelRequestSpan {
    capture_headers: Arc<[HeaderName]>,
}

impl OtelRequestSpan {
    /// Build the span shape, capturing the given inbound headers as
    /// `http.request.header.<name>` attributes.
    #[must_use]
    pub fn new(capture_headers: Arc<[HeaderName]>) -> Self {
        Self { capture_headers }
    }
}

impl MakeRequestSpan for OtelRequestSpan {
    fn make_span(
        &self,
        req: &RequestHead<'_>,
        route: &str,
        request_id: Option<&str>,
    ) -> tracing::Span {
        // `http.route` is the matched route template (OTel semconv), never the
        // raw path: backends tag on it, so raw paths would mint one tag value
        // per unique scanner URL. The raw path stays opt-in on the summary
        // event (`trace.record-path`).
        let span = tracing::info_span!(
            target: "r2e::http",
            "request",
            http.request.method = method_label(req.method),
            http.route = route,
            http.response.status_code = tracing::field::Empty,
            request_id = tracing::field::Empty,
            otel.kind = "server",
        );

        if let Some(id) = request_id {
            span.record("request_id", id);
        }

        let parent_cx = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers))
        });
        // `Err` only when no OTel layer is installed on the subscriber — the
        // span is still perfectly usable for `fmt` output.
        let _ = span.set_parent(parent_cx);

        for name in self.capture_headers.iter() {
            if let Some(value) = req.headers.get(name).and_then(|v| v.to_str().ok()) {
                span.set_attribute(format!("http.request.header.{name}"), value.to_owned());
            }
        }

        span
    }

    fn on_response(&self, span: &tracing::Span, outcome: &RequestOutcome<'_>) {
        span.record(
            "http.response.status_code",
            outcome.status.map(|status| status.as_u16()),
        );
        // The summary event (level split, `latency_ms`, opt-in path/query) is
        // the same one the default shape emits — only the span differs.
        r2e_core::runtime::http_trace::default_on_response(span, outcome);
    }
}
