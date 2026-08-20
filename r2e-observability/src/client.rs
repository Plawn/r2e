//! Outgoing HTTP instrumentation: one OTel **client** span per request plus
//! W3C trace-context propagation.
//!
//! Two levels of integration:
//!
//! - [`traced_reqwest_client`] / [`TraceContextMiddleware`] — the full
//!   picture. Every outgoing request opens a span with `otel.kind = "client"`
//!   (OTel HTTP-client semantic conventions: `http.request.method`,
//!   `server.address`, `server.port`, `url.full`, `http.response.status_code`,
//!   `otel.status_code`/`error.message` on failure) and the injected
//!   `traceparent` carries **that span's** id. This is what tracing backends
//!   (Tempo metrics-generator, Jaeger, Grafana service graph) need to draw a
//!   `caller → callee` edge: they pair a CLIENT span with its direct SERVER
//!   child.
//! - [`inject_current_context`] — header injection only, from whatever span
//!   is current. Use it when you already manage a client span yourself (or
//!   for non-reqwest transports). Used on its own, **no client span is
//!   emitted**: the trace is still joined across services, but the service
//!   graph shows no edge between them.
//!
//! Implemented on top of [`reqwest_tracing`] (pinned to the same
//! `opentelemetry` / `tracing-opentelemetry` versions as this crate, so the
//! global propagator installed by [`crate::Observability`] is the one used —
//! a version skew would make injection a silent no-op).
//!
//! Per-request knobs from `reqwest_tracing` work unchanged through
//! [`reqwest_middleware::RequestBuilder::with_extension`]:
//! [`OtelName`] (fixed span name), [`OtelPathNames`] (low-cardinality
//! `{method} {route}` names), [`DisableOtelPropagation`].

use std::borrow::Cow;

use http::Extensions;
use opentelemetry::propagation::Injector;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Request, Response};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware, Next};
use reqwest_tracing::{
    default_on_request_end, reqwest_otel_span, ReqwestOtelSpanBackend, TracingMiddleware,
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub use reqwest_tracing::{DisableOtelPropagation, OtelName, OtelPathNames};

/// Inject the current tracing span's OpenTelemetry context into HTTP headers.
///
/// With the W3C propagator installed by [`crate::Observability`], this adds
/// `traceparent` and, when present, `tracestate`. It is a no-op when there is
/// no active valid OpenTelemetry span.
///
/// This is the low-level building block: it does **not** open a client span,
/// so the propagated id is the *current* span's (typically the caller's
/// server span). Backends that derive a service graph from CLIENT→SERVER
/// pairs will not draw an edge for calls instrumented this way — wrap the
/// call in your own `otel.kind = "client"` span, or use
/// [`traced_reqwest_client`], which does both.
pub fn inject_current_context(headers: &mut HeaderMap) {
    let context = tracing::Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
}

/// Span backend following the OTel HTTP-client semantic conventions.
///
/// Span name is `HTTP {method}` by default — per spec, the name must stay
/// low-cardinality, so the URL is never part of it. Opt into a route template
/// with [`OtelPathNames`] (`{method} {route}`) or a fixed [`OtelName`]. The
/// full URL (credentials stripped) is recorded as `url.full`.
#[derive(Debug, Clone, Copy, Default)]
pub struct R2eSpanBackend;

impl ReqwestOtelSpanBackend for R2eSpanBackend {
    fn on_request_start(req: &Request, ext: &mut Extensions) -> tracing::Span {
        let name = span_name(req, ext);
        let url = url_without_credentials(req.url());
        reqwest_otel_span!(name = name, req, url.full = %url)
    }

    fn on_request_end(
        span: &tracing::Span,
        outcome: &reqwest_middleware::Result<Response>,
        _: &mut Extensions,
    ) {
        default_on_request_end(span, outcome)
    }
}

fn span_name<'a>(req: &'a Request, ext: &'a Extensions) -> Cow<'a, str> {
    if let Some(name) = ext.get::<OtelName>() {
        Cow::Borrowed(name.0.as_ref())
    } else if let Some(paths) = ext.get::<OtelPathNames>() {
        match paths.find(req.url().path()) {
            Some(route) => Cow::Owned(format!("{} {route}", req.method())),
            None => Cow::Owned(format!("HTTP {}", req.method())),
        }
    } else {
        Cow::Owned(format!("HTTP {}", req.method()))
    }
}

fn url_without_credentials(url: &url::Url) -> url::Url {
    let mut url = url.clone();
    if url.username().is_empty() && url.password().is_none() {
        return url;
    }
    // Both setters only fail for cannot-be-a-base URLs, which never carry
    // credentials in the first place.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url
}

/// Reqwest middleware that opens an OTel **client** span per request and
/// propagates that span's context (`traceparent`/`tracestate`).
///
/// See the [module docs](self) for the attributes recorded and why the client
/// span matters for service graphs.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceContextMiddleware;

#[async_trait::async_trait]
impl Middleware for TraceContextMiddleware {
    async fn handle(
        &self,
        request: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        TracingMiddleware::<R2eSpanBackend>::new()
            .handle(request, extensions, next)
            .await
    }
}

/// Wrap a reqwest client so every outgoing request runs in a client span and
/// propagates trace context.
///
/// Pass the returned client to SDKs that accept a
/// [`ClientWithMiddleware`]. No per-request instrumentation is required.
pub fn traced_reqwest_client(client: reqwest::Client) -> ClientWithMiddleware {
    ClientBuilder::new(client)
        .with(TraceContextMiddleware)
        .build()
}

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            return;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            return;
        };
        self.0.insert(name, value);
    }
}
