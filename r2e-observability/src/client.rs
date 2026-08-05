//! Outgoing HTTP trace-context propagation.

use http::Extensions;
use opentelemetry::propagation::Injector;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Request, Response};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware, Next};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Inject the current tracing span's OpenTelemetry context into HTTP headers.
///
/// With the W3C propagator installed by [`crate::Observability`], this adds
/// `traceparent` and, when present, `tracestate`. It is a no-op when there is
/// no active valid OpenTelemetry span.
pub fn inject_current_context(headers: &mut HeaderMap) {
    let context = tracing::Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
}

/// Reqwest middleware that propagates the current OpenTelemetry context.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceContextMiddleware;

#[async_trait::async_trait]
impl Middleware for TraceContextMiddleware {
    async fn handle(
        &self,
        mut request: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        inject_current_context(request.headers_mut());
        next.run(request, extensions).await
    }
}

/// Wrap a reqwest client so every outgoing request propagates trace context.
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
