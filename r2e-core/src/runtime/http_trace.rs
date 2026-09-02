//! The per-request HTTP tracing layer behind the
//! [`HttpTrace`](crate::builtins::HttpTrace) plugin.
//!
//! One span per request (name `request`, target `r2e::http`), entered for the
//! whole handler future — so `request_id` and `route` decorate every log line
//! a handler emits — plus one summary event when the response head is ready.
//!
//! The span itself is built through [`MakeRequestSpan`], which is the seam
//! `r2e-observability` uses to swap in OpenTelemetry semconv field names and a
//! parent context extracted from the inbound `traceparent`, while keeping the
//! exclusions, request-id handling and summary event of this layer. That is
//! what makes it **one** span per request instead of the historical
//! tower-http span + OTel span pair.

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use pin_project_lite::pin_project;
use tower::{Layer, Service};

use crate::http::extract::MatchedPath;
use crate::http::header::HttpRequest as Request;
use crate::http::labels::{method_label, path_excluded, route_label};
use crate::http::response::Response;
use crate::http::{HeaderName, HeaderValue, StatusCode};
use crate::web::request_head::RequestHead;

/// `tracing` target of the request span and its summary event.
pub const TRACE_TARGET: &str = "r2e::http";

/// Name of the per-request span.
pub const SPAN_NAME: &str = "request";

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Resolved per-request tracing contract — what the layer actually does, after
/// the [`HttpTrace`](crate::builtins::HttpTrace) plugin has merged the explicit
/// builder settings, the app's `trace:` section, an optional preset and the
/// built-in defaults.
///
/// Both list fields are `Arc<[..]>`: the HTTP backend clones the wrapped
/// service once per request, and a `Vec` there would deep-clone every
/// configured entry every time.
#[derive(Clone, Debug)]
pub struct HttpTraceSettings {
    /// Path prefixes excluded from tracing entirely — no span, no event.
    /// Matched against the raw path **and** the bounded route label.
    pub exclude_paths: Arc<[String]>,
    /// Resolve (or mint) a request id, put it on the span and echo it back as
    /// `x-request-id`.
    pub request_id: bool,
    /// Put the raw request path on the summary **event** (never on the span).
    pub record_path: bool,
    /// Put the raw query string on the summary **event** (never on the span).
    pub record_query: bool,
    /// Inbound headers recorded on the span, pre-validated at build time.
    pub capture_headers: Arc<[HeaderName]>,
    /// Emit the one-line `request completed` summary event.
    pub summary: bool,
    /// Emit an additional `request started` event at DEBUG.
    pub request_event: bool,
}

impl Default for HttpTraceSettings {
    fn default() -> Self {
        Self {
            exclude_paths: Arc::from(Vec::new()),
            request_id: true,
            record_path: false,
            record_query: false,
            capture_headers: Arc::from(Vec::new()),
            summary: true,
            request_event: false,
        }
    }
}

/// What the layer measured for one request, handed to
/// [`MakeRequestSpan::on_response`].
#[derive(Debug)]
pub struct RequestOutcome<'a> {
    /// Response status, or `None` when the inner service returned a transport
    /// error. Unreachable under the HTTP backend's `Infallible` services; kept
    /// for the generic `tower` contract.
    pub status: Option<StatusCode>,
    /// Time to the response **head**. Streaming bodies are not included — the
    /// reason the Prometheus layer keeps its own timer.
    pub latency: Duration,
    /// Raw request path, `Some` only when `record-path` is on.
    pub path: Option<&'a str>,
    /// Raw query string, `Some` only when `record-query` is on and the request
    /// had one.
    pub query: Option<&'a str>,
    /// Whether the configured contract asks for the summary event
    /// (`trace.summary`). Honoured by [`default_on_response`]; a custom
    /// implementation that emits its own event should honour it too.
    pub emit_summary: bool,
}

impl RequestOutcome<'_> {
    /// Latency to the response head, in milliseconds.
    #[must_use]
    pub fn latency_ms(&self) -> f64 {
        self.latency.as_secs_f64() * 1000.0
    }

    /// Whether this request should be reported at ERROR level (5xx, or a
    /// transport error).
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.status.is_none_or(|s| s.is_server_error())
    }
}

/// Builds the span for one request — the extension point of the layer.
///
/// Everything else (exclusions, request-id resolution and echo, timing, the
/// summary event) stays with [`HttpTraceLayer`]; an implementation replaces
/// only the span shape.
///
/// # Field names
///
/// `tracing` span fields come from a **static** callsite, so an implementation
/// declares its field set at compile time. Runtime-named fields (one per
/// configured `capture-headers` entry, say) are not expressible: see
/// [`DefaultRequestSpan`] for what it does instead.
pub trait MakeRequestSpan: Send + Sync + 'static {
    /// Build the span for one request. `route` is the bounded route label and
    /// `request_id` is already resolved (`None` when `trace.request-id` is
    /// off).
    fn make_span(
        &self,
        req: &RequestHead<'_>,
        route: &str,
        request_id: Option<&str>,
    ) -> tracing::Span;

    /// Record the outcome on the span **and** emit the summary event.
    ///
    /// One method owns both the field names and the event shape on purpose:
    /// there is deliberately no "the layer records `status` by name, declare it
    /// `Empty`" contract, so a custom span with different field names overrides
    /// this too and nothing is lost silently.
    fn on_response(&self, span: &tracing::Span, outcome: &RequestOutcome<'_>) {
        default_on_response(span, outcome);
    }
}

/// The default outcome handling: record `status` on the span (a no-op for a
/// span that does not declare the field) and emit the one-line summary event
/// inside it — INFO below 500, ERROR at 5xx and on a transport error.
pub fn default_on_response(span: &tracing::Span, outcome: &RequestOutcome<'_>) {
    let status = outcome.status.map(|s| s.as_u16());
    span.record("status", status);

    if !outcome.emit_summary {
        return;
    }

    let _enter = span.enter();
    let latency_ms = outcome.latency_ms();
    if outcome.is_failure() {
        tracing::error!(
            target: TRACE_TARGET,
            status,
            latency_ms,
            path = outcome.path,
            query = outcome.query,
            "request completed"
        );
    } else {
        tracing::info!(
            target: TRACE_TARGET,
            status,
            latency_ms,
            path = outcome.path,
            query = outcome.query,
            "request completed"
        );
    }
}

/// The built-in span shape: `method`, `route`, `request_id`, `status`.
///
/// `capture-headers` entries are recorded together in a single `headers` field
/// as `name=value` pairs separated by spaces. `tracing` derives a span's field
/// names from its `&'static` callsite metadata, so a field per configured
/// header name (`header.user-agent`, …) cannot be built at runtime; the
/// OpenTelemetry span in `r2e-observability` — whose attributes are a runtime
/// map — does get one attribute per header.
#[derive(Clone, Debug, Default)]
pub struct DefaultRequestSpan {
    capture_headers: Arc<[HeaderName]>,
}

impl DefaultRequestSpan {
    /// Build the default span maker, capturing the given inbound headers.
    #[must_use]
    pub fn new(capture_headers: Arc<[HeaderName]>) -> Self {
        Self { capture_headers }
    }
}

/// Render the configured `capture-headers` of one request as
/// `name=value name=value`, or `None` when none of them is present.
///
/// Public so an alternative [`MakeRequestSpan`] on the `fmt` side can reuse the
/// exact same rendering.
#[must_use]
pub fn captured_headers(req: &RequestHead<'_>, capture_headers: &[HeaderName]) -> Option<String> {
    if capture_headers.is_empty() {
        return None;
    }
    let mut out = String::new();
    for name in capture_headers {
        let Some(value) = req.headers.get(name).and_then(|v| v.to_str().ok()) else {
            continue;
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name.as_str());
        out.push('=');
        out.push_str(value);
    }
    (!out.is_empty()).then_some(out)
}

impl MakeRequestSpan for DefaultRequestSpan {
    fn make_span(
        &self,
        req: &RequestHead<'_>,
        route: &str,
        request_id: Option<&str>,
    ) -> tracing::Span {
        let span = tracing::info_span!(
            target: "r2e::http",
            "request",
            method = method_label(req.method),
            route = route,
            request_id = tracing::field::Empty,
            headers = tracing::field::Empty,
            status = tracing::field::Empty,
        );
        if let Some(id) = request_id {
            span.record("request_id", id);
        }
        if let Some(headers) = captured_headers(req, &self.capture_headers) {
            span.record("headers", headers.as_str());
        }
        span
    }
}

/// Tower layer emitting one span + one summary event per request.
///
/// `M` is the [`MakeRequestSpan`] implementation; it is `?Sized` so the plugin
/// can hold an `Arc<dyn MakeRequestSpan>` while `r2e-observability` keeps
/// static dispatch on its own type.
pub struct HttpTraceLayer<M: ?Sized = DefaultRequestSpan> {
    settings: Arc<HttpTraceSettings>,
    make_span: Arc<M>,
}

impl HttpTraceLayer<DefaultRequestSpan> {
    /// The layer with the built-in [`DefaultRequestSpan`] shape.
    #[must_use]
    pub fn new(settings: HttpTraceSettings) -> Self {
        let make_span = DefaultRequestSpan::new(Arc::clone(&settings.capture_headers));
        Self::with_make_span(settings, make_span)
    }
}

impl<M: MakeRequestSpan> HttpTraceLayer<M> {
    /// The layer with a custom span shape.
    #[must_use]
    pub fn with_make_span(settings: HttpTraceSettings, make_span: M) -> Self {
        Self::from_shared(settings, Arc::new(make_span))
    }
}

impl<M: MakeRequestSpan + ?Sized> HttpTraceLayer<M> {
    /// The layer with an already-shared span shape — the erased
    /// (`Arc<dyn MakeRequestSpan>`) form the plugin builder uses.
    #[must_use]
    pub fn from_shared(settings: HttpTraceSettings, make_span: Arc<M>) -> Self {
        Self {
            settings: Arc::new(settings),
            make_span,
        }
    }
}

impl<M: ?Sized> Clone for HttpTraceLayer<M> {
    fn clone(&self) -> Self {
        Self {
            settings: Arc::clone(&self.settings),
            make_span: Arc::clone(&self.make_span),
        }
    }
}

impl<S, M: ?Sized> Layer<S> for HttpTraceLayer<M> {
    type Service = HttpTraceService<S, M>;

    fn layer(&self, inner: S) -> Self::Service {
        HttpTraceService {
            inner,
            settings: Arc::clone(&self.settings),
            make_span: Arc::clone(&self.make_span),
        }
    }
}

/// The service produced by [`HttpTraceLayer`].
pub struct HttpTraceService<S, M: ?Sized = DefaultRequestSpan> {
    inner: S,
    settings: Arc<HttpTraceSettings>,
    make_span: Arc<M>,
}

impl<S: Clone, M: ?Sized> Clone for HttpTraceService<S, M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            settings: Arc::clone(&self.settings),
            make_span: Arc::clone(&self.make_span),
        }
    }
}

/// Per-request state kept by the response future for a **traced** request.
struct Traced<M: ?Sized> {
    span: tracing::Span,
    make_span: Arc<M>,
    settings: Arc<HttpTraceSettings>,
    start: Instant,
    /// The id to echo back as `x-request-id`, when `trace.request-id` is on.
    request_id: Option<HeaderValue>,
    /// Raw path/query, captured only when the matching knob is on so an
    /// excluded-by-default secret never even reaches the future.
    path: Option<String>,
    query: Option<String>,
}

impl<S, M, ReqBody, ResBody> Service<Request<ReqBody>> for HttpTraceService<S, M>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    M: MakeRequestSpan + ?Sized,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = HttpTraceResponseFuture<S::Future, M>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        // 1. Exclusions — prefix match on the raw path OR the bounded route
        //    label, the same semantics as `prometheus.exclude-paths`. An
        //    excluded request is passed through untouched: no span, no event,
        //    no request id, so handler logs under `/health` are not even
        //    decorated.
        let matched_path = req.extensions().get::<MatchedPath>().cloned();
        let label = route_label(matched_path.as_ref());
        if path_excluded(req.uri().path(), label, &self.settings.exclude_paths) {
            return HttpTraceResponseFuture {
                inner: self.inner.call(req),
                traced: None,
            };
        }

        // 2. Request id: reuse whatever already resolved one (`RequestIdPlugin`
        //    installed outside us), else the inbound header, else mint. The
        //    resolved id goes back on the *request* as both the extension and
        //    the header, so a `RequestIdPlugin` installed inside us agrees
        //    instead of minting a second id — install order does not matter.
        let request_id = self
            .settings
            .request_id
            .then(|| resolve_request_id(&mut req));

        // 3. Build the span from the R2E-owned view of the request head.
        let peer_addr = req
            .extensions()
            .get::<crate::http::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0);
        let head = RequestHead {
            method: req.method(),
            uri: req.uri(),
            headers: req.headers(),
            extensions: req.extensions(),
            // Route matching happened, but the backend keeps its decoded
            // params private until extraction; a layer sees none.
            path_params: crate::decorators::guards::PathParams::EMPTY,
            peer_addr,
        };
        let label = route_label(req.extensions().get::<MatchedPath>());
        let span =
            self.make_span
                .make_span(&head, label, request_id.as_ref().map(|(id, _)| id.as_str()));

        let path = self
            .settings
            .record_path
            .then(|| req.uri().path().to_owned());
        let query = self
            .settings
            .record_query
            .then(|| req.uri().query().map(str::to_owned))
            .flatten();

        if self.settings.request_event {
            let _enter = span.enter();
            tracing::debug!(target: TRACE_TARGET, "request started");
        }

        HttpTraceResponseFuture {
            inner: self.inner.call(req),
            traced: Some(Traced {
                span,
                make_span: Arc::clone(&self.make_span),
                settings: Arc::clone(&self.settings),
                start: Instant::now(),
                request_id: request_id.map(|(_, header)| header),
                path,
                query,
            }),
        }
    }
}

/// Resolve the request id for one request and make it visible to everything
/// downstream: the [`RequestId`](crate::builtins::request_id::RequestId)
/// extension and the `x-request-id` request header.
fn resolve_request_id<B>(req: &mut Request<B>) -> (String, HeaderValue) {
    use crate::builtins::request_id::{fresh_request_id, RequestId};

    let resolved = req
        .extensions()
        .get::<RequestId>()
        .and_then(|id| HeaderValue::from_str(&id.0).ok().map(|h| (id.0.clone(), h)))
        .or_else(|| {
            req.headers()
                .get(&X_REQUEST_ID)
                .and_then(|value| value.to_str().ok().map(|s| (s.to_owned(), value.clone())))
        });

    let (id, header) = resolved.unwrap_or_else(fresh_request_id);
    req.extensions_mut().insert(RequestId(id.clone()));
    req.headers_mut()
        .insert(X_REQUEST_ID.clone(), header.clone());
    (id, header)
}

pin_project! {
    /// Response future of [`HttpTraceService`]: enters the span while the
    /// handler runs, then hands the outcome to [`MakeRequestSpan::on_response`]
    /// and echoes `x-request-id`.
    pub struct HttpTraceResponseFuture<F, M: ?Sized> {
        #[pin]
        inner: F,
        // `None` for an excluded request — a pure pass-through.
        traced: Option<Traced<M>>,
    }
}

impl<F, M, ResBody, E> Future for HttpTraceResponseFuture<F, M>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
    M: MakeRequestSpan + ?Sized,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let Some(traced) = this.traced.as_mut() else {
            return this.inner.poll(cx);
        };

        // The span is entered for the whole handler future — that is what puts
        // `request_id` / `route` on every log line the handler emits.
        let mut result = {
            let _enter = traced.span.enter();
            match this.inner.poll(cx) {
                Poll::Ready(result) => result,
                Poll::Pending => return Poll::Pending,
            }
        };

        let outcome = RequestOutcome {
            status: result.as_ref().ok().map(|resp| resp.status()),
            latency: traced.start.elapsed(),
            path: traced.path.as_deref(),
            query: traced.query.as_deref(),
            emit_summary: traced.settings.summary,
        };
        traced.make_span.on_response(&traced.span, &outcome);

        if let (Ok(response), Some(id)) = (result.as_mut(), traced.request_id.as_ref()) {
            response
                .headers_mut()
                .insert(X_REQUEST_ID.clone(), id.clone());
        }

        Poll::Ready(result)
    }
}
