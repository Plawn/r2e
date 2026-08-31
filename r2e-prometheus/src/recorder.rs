//! The recorder seam: *what* the HTTP tracking layer records into.
//!
//! The layer ([`crate::HttpMetricsLayer`]) owns the request-side semantics —
//! which requests are tracked, the bounded `method`/`path` labels, the
//! cancellation-safe in-flight gauge — and delegates the actual metric writes
//! to an [`HttpMetricsRecorder`]. Two implementations ship:
//!
//! - [`PrometheusRecorder`] (default): writes into this crate's global
//!   `prometheus::Registry`, which the [`crate::Prometheus`] plugin also
//!   exposes at `/metrics`.
//! - [`crate::MetricsFacadeRecorder`] (feature `metrics-facade`): writes
//!   through the [`metrics`](https://docs.rs/metrics) facade macros, so an
//!   application that already owns a `metrics` recorder/exporter gets R2E's
//!   HTTP metrics without adopting the `prometheus` crate.

/// Sink for the HTTP request metrics produced by [`crate::HttpMetricsLayer`].
///
/// Implementations must be cheap to clone: the layer clones the recorder into
/// every wrapped service and into every in-flight guard.
pub trait HttpMetricsRecorder: Clone + Send + Sync + 'static {
    /// Record one completed (or failed) request.
    ///
    /// `method` and `path` are already bounded label values
    /// (`r2e_core::http::labels`); `path` is a route template or the
    /// `unmatched` sentinel.
    fn record_request(&self, method: &'static str, path: &str, status: u16, duration_secs: f64);

    /// Increment the concurrent-requests gauge.
    fn inc_in_flight(&self);

    /// Decrement the concurrent-requests gauge.
    fn dec_in_flight(&self);
}

/// Records into this crate's process-global `prometheus::Registry` — the
/// historical (and default) behavior of [`crate::PrometheusLayer`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PrometheusRecorder;

impl HttpMetricsRecorder for PrometheusRecorder {
    fn record_request(&self, method: &'static str, path: &str, status: u16, duration_secs: f64) {
        crate::metrics::record_request(method, path, status, duration_secs);
    }

    fn inc_in_flight(&self) {
        crate::metrics::inc_in_flight();
    }

    fn dec_in_flight(&self) {
        crate::metrics::dec_in_flight();
    }
}
