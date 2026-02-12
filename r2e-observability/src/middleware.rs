use http::Request;
use opentelemetry::propagation::Extractor;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tower::{Layer, Service};

/// Header extractor for OpenTelemetry propagation.
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// Tower layer that extracts trace context from incoming HTTP headers
/// and creates a tracing span for each request.
#[derive(Clone)]
pub struct OtelTraceLayer {
    capture_headers: Vec<String>,
}

impl OtelTraceLayer {
    pub fn new(capture_headers: Vec<String>) -> Self {
        Self { capture_headers }
    }
}

impl<S> Layer<S> for OtelTraceLayer {
    type Service = OtelTraceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OtelTraceService {
            inner,
            capture_headers: self.capture_headers.clone(),
        }
    }
}

/// Tower service that wraps requests with OpenTelemetry trace context.
#[derive(Clone)]
pub struct OtelTraceService<S> {
    inner: S,
    capture_headers: Vec<String>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for OtelTraceService<S>
where
    S: Service<Request<ReqBody>, Response = http::Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = OtelResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        // Extract parent context from incoming headers
        let _parent_cx = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });

        let method = req.method().to_string();
        let path = req.uri().path().to_string();

        // Create the request span
        let span = tracing::info_span!(
            "HTTP request",
            http.method = %method,
            http.route = %path,
            http.status_code = tracing::field::Empty,
            otel.kind = "server",
        );

        // Log captured headers as span events
        for name in &self.capture_headers {
            if let Some(val) = req.headers().get(name.as_str()) {
                if let Ok(s) = val.to_str() {
                    tracing::debug!(parent: &span, header.name = %name, header.value = %s, "captured header");
                }
            }
        }

        OtelResponseFuture {
            inner: self.inner.call(req),
            span,
            _start: Instant::now(),
        }
    }
}

pin_project! {
    /// Future that records trace information when the response completes.
    pub struct OtelResponseFuture<F> {
        #[pin]
        inner: F,
        span: tracing::Span,
        _start: Instant,
    }
}

impl<F, ResBody, E> Future for OtelResponseFuture<F>
where
    F: Future<Output = Result<http::Response<ResBody>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _enter = this.span.enter();

        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                if let Ok(ref response) = result {
                    this.span
                        .record("http.status_code", response.status().as_u16());
                }
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
