use crate::metrics::{dec_in_flight, inc_in_flight, record_request, MetricsConfig};
use http::{Request, Response};
use pin_project_lite::pin_project;
use r2e_core::http::extract::MatchedPath;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tower::{Layer, Service};

/// `path` label value for requests no route matched (404s and
/// `Router::fallback`-handled requests carry no [`MatchedPath`]).
/// A single sentinel keeps label cardinality bounded under arbitrary-path
/// scanner traffic.
pub const UNMATCHED_PATH_LABEL: &str = "unmatched";

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

        // Exclusion matches on the raw request path (e.g. "/metrics", "/health").
        let raw_path = req.uri().path();
        let should_track = !self
            .config
            .exclude_paths
            .iter()
            .any(|p| raw_path.starts_with(p));

        // Label with the matched route template ("/users/{id}") — bounded by the
        // number of registered routes. Unmatched requests collapse into one
        // sentinel value instead of minting a series per unique URL.
        let path = req
            .extensions()
            .get::<MatchedPath>()
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| UNMATCHED_PATH_LABEL.to_string());

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

                    record_request(this.method, this.path, status, duration);
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
