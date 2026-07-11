use crate::metrics::{dec_in_flight, inc_in_flight, record_request, MetricsConfig};
use http::{Request, Response};
use pin_project_lite::pin_project;
use r2e_core::http::extract::MatchedPath;
use r2e_core::http::labels::{method_label, route_label};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tower::{Layer, Service};

// Label-bounding semantics are shared with r2e-observability via
// `r2e_core::http::labels`; re-exported here so the crate's public API keeps
// exposing the sentinel values it records under.
pub use r2e_core::http::labels::{OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL};

/// RAII balance for the in-flight gauge: incremented on creation, decremented
/// on drop. The response future can be dropped without ever completing (client
/// disconnect cancels the request mid-flight), so pairing the decrement with
/// `Poll::Ready` alone would leak the gauge upward.
struct InFlightGuard;

impl InFlightGuard {
    fn new() -> Self {
        inc_in_flight();
        Self
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        dec_in_flight();
    }
}

/// Tower layer that tracks HTTP request metrics.
#[derive(Clone)]
pub struct PrometheusLayer {
    config: MetricsConfig,
}

impl PrometheusLayer {
    pub fn new(config: MetricsConfig) -> Self {
        Self { config }
    }
}

impl<S> Layer<S> for PrometheusLayer {
    type Service = PrometheusService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PrometheusService {
            inner,
            config: self.config.clone(),
        }
    }
}

/// Tower service that wraps requests with metrics tracking.
#[derive(Clone)]
pub struct PrometheusService<S> {
    inner: S,
    config: MetricsConfig,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for PrometheusService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = PrometheusResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let method = method_label(req.method());

        // Label with the matched route template ("/users/{id}") — bounded by the
        // number of registered routes. Unmatched requests collapse into one
        // sentinel value instead of minting a series per unique URL.
        // `MatchedPath` clones by `Arc` refcount bump — no per-request allocation.
        let matched_path = req.extensions().get::<MatchedPath>().cloned();

        // Exclusion prefix-matches both the raw request path ("/users/5") and
        // the label the request would be recorded under ("/users/{id}" or the
        // sentinel), so either spelling in `exclude_paths` works.
        let raw_path = req.uri().path();
        let label_path = route_label(matched_path.as_ref());
        let should_track = !self
            .config
            .exclude_paths
            .iter()
            .any(|p| raw_path.starts_with(p) || label_path.starts_with(p));

        PrometheusResponseFuture {
            inner: self.inner.call(req),
            method,
            matched_path,
            start: Instant::now(),
            in_flight: should_track.then(InFlightGuard::new),
        }
    }
}

pin_project! {
    /// Future that records metrics when the response completes.
    pub struct PrometheusResponseFuture<F> {
        #[pin]
        inner: F,
        method: &'static str,
        matched_path: Option<MatchedPath>,
        start: Instant,
        // `Some` while the request is tracked and in flight; dropping it
        // (normal completion or cancellation) decrements the gauge.
        in_flight: Option<InFlightGuard>,
    }
}

impl<F, ResBody, E> Future for PrometheusResponseFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                if this.in_flight.take().is_some() {
                    let duration = this.start.elapsed().as_secs_f64();
                    let status = match &result {
                        Ok(response) => response.status().as_u16(),
                        Err(_) => 500,
                    };

                    let path = route_label(this.matched_path.as_ref());
                    record_request(this.method, path, status, duration);
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
