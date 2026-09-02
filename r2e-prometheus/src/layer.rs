use crate::metrics::MetricsConfig;
use crate::recorder::{HttpMetricsRecorder, PrometheusRecorder};
use http::{Request, Response};
use pin_project_lite::pin_project;
use r2e_core::http::extract::MatchedPath;
use r2e_core::http::labels::{method_label, path_excluded, route_label};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
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
struct InFlightGuard<R: HttpMetricsRecorder> {
    recorder: R,
}

impl<R: HttpMetricsRecorder> InFlightGuard<R> {
    fn new(recorder: R) -> Self {
        recorder.inc_in_flight();
        Self { recorder }
    }
}

impl<R: HttpMetricsRecorder> Drop for InFlightGuard<R> {
    fn drop(&mut self) {
        self.recorder.dec_in_flight();
    }
}

/// Tower layer that tracks HTTP request metrics, recording into `R`.
///
/// The default recorder is [`PrometheusRecorder`] (this crate's global
/// `prometheus::Registry`); [`PrometheusLayer`] is the alias for that
/// combination and keeps the historical constructor
/// (`PrometheusLayer::new(config)`).
#[derive(Clone)]
pub struct HttpMetricsLayer<R = PrometheusRecorder> {
    config: Arc<MetricsConfig>,
    recorder: R,
}

/// The HTTP tracking layer backed by this crate's `prometheus` registry.
pub type PrometheusLayer = HttpMetricsLayer<PrometheusRecorder>;

impl HttpMetricsLayer<PrometheusRecorder> {
    pub fn new(config: MetricsConfig) -> Self {
        Self::with_recorder(config, PrometheusRecorder)
    }
}

impl<R: HttpMetricsRecorder> HttpMetricsLayer<R> {
    /// Build the tracking layer over an explicit recorder.
    pub fn with_recorder(config: MetricsConfig, recorder: R) -> Self {
        Self {
            config: Arc::new(config),
            recorder,
        }
    }
}

impl<S, R: HttpMetricsRecorder> Layer<S> for HttpMetricsLayer<R> {
    type Service = HttpMetricsService<S, R>;

    fn layer(&self, inner: S) -> Self::Service {
        HttpMetricsService {
            inner,
            config: self.config.clone(),
            recorder: self.recorder.clone(),
        }
    }
}

/// Tower service that wraps requests with metrics tracking.
#[derive(Clone)]
pub struct HttpMetricsService<S, R = PrometheusRecorder> {
    inner: S,
    config: Arc<MetricsConfig>,
    recorder: R,
}

/// The tracking service backed by this crate's `prometheus` registry.
pub type PrometheusService<S> = HttpMetricsService<S, PrometheusRecorder>;

impl<S, R, ReqBody, ResBody> Service<Request<ReqBody>> for HttpMetricsService<S, R>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    R: HttpMetricsRecorder,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = HttpMetricsResponseFuture<S::Future, R>;

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
        let should_track = !path_excluded(raw_path, label_path, &self.config.exclude_paths);

        HttpMetricsResponseFuture {
            inner: self.inner.call(req),
            method,
            matched_path,
            start: Instant::now(),
            in_flight: should_track.then(|| InFlightGuard::new(self.recorder.clone())),
        }
    }
}

pin_project! {
    /// Future that records metrics when the response completes.
    pub struct HttpMetricsResponseFuture<F, R: HttpMetricsRecorder> {
        #[pin]
        inner: F,
        method: &'static str,
        matched_path: Option<MatchedPath>,
        start: Instant,
        // `Some` while the request is tracked and in flight; dropping it
        // (normal completion or cancellation) decrements the gauge.
        in_flight: Option<InFlightGuard<R>>,
    }
}

/// The response future backed by this crate's `prometheus` registry.
pub type PrometheusResponseFuture<F> = HttpMetricsResponseFuture<F, PrometheusRecorder>;

impl<F, R, ResBody, E> Future for HttpMetricsResponseFuture<F, R>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
    R: HttpMetricsRecorder,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                if let Some(guard) = this.in_flight.take() {
                    let duration = this.start.elapsed().as_secs_f64();
                    let status = match &result {
                        Ok(response) => response.status().as_u16(),
                        Err(_) => 500,
                    };

                    let path = route_label(this.matched_path.as_ref());
                    guard
                        .recorder
                        .record_request(this.method, path, status, duration);
                    // `guard` drops here: the gauge decrement stays paired with
                    // the increment done in `call`.
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
