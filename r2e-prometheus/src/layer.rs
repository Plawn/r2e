use crate::metrics::{dec_in_flight, inc_in_flight, record_request, MetricsConfig};
use http::{Request, Response};
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tower::{Layer, Service};

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
        let method = req.method().to_string();
        let path = req.uri().path().to_string();

        // Check if this path should be excluded
        let should_track = !self
            .config
            .exclude_paths
            .iter()
            .any(|p| path.starts_with(p));

        if should_track {
            inc_in_flight();
        }

        PrometheusResponseFuture {
            inner: self.inner.call(req),
            method,
            path,
            start: Instant::now(),
            should_track,
        }
    }
}

pin_project! {
    /// Future that records metrics when the response completes.
    pub struct PrometheusResponseFuture<F> {
        #[pin]
        inner: F,
        method: String,
        path: String,
        start: Instant,
        should_track: bool,
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
                if *this.should_track {
                    dec_in_flight();

                    let duration = this.start.elapsed().as_secs_f64();
                    let status = match &result {
                        Ok(response) => response.status().as_u16(),
                        Err(_) => 500,
                    };

                    // Normalize path to avoid cardinality explosion
                    let normalized_path = normalize_path(this.path);
                    record_request(this.method, &normalized_path, status, duration);
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Normalize path to prevent high cardinality.
/// Replaces numeric path segments with placeholders.
fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.parse::<i64>().is_ok() || is_uuid(segment) {
                "{id}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Check if a string looks like a UUID.
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
        && s.matches('-').count() == 4
}
