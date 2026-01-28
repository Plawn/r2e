use prometheus::{
    exponential_buckets, histogram_opts, opts, Encoder, HistogramVec, IntCounterVec, IntGauge,
    Registry, TextEncoder,
};
use std::sync::OnceLock;

static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Global metrics instance.
pub struct Metrics {
    pub registry: Registry,
    pub http_requests_total: IntCounterVec,
    pub http_request_duration_seconds: HistogramVec,
    pub http_requests_in_flight: IntGauge,
}

impl Metrics {
    fn new(config: &MetricsConfig) -> Self {
        let registry = Registry::new();

        let prefix = config
            .namespace
            .as_ref()
            .map(|s| format!("{}_", s))
            .unwrap_or_default();

        let http_requests_total = IntCounterVec::new(
            opts!(
                format!("{}http_requests_total", prefix),
                "Total number of HTTP requests"
            ),
            &["method", "path", "status"],
        )
        .expect("metric can be created");

        let http_request_duration_seconds = HistogramVec::new(
            histogram_opts!(
                format!("{}http_request_duration_seconds", prefix),
                "HTTP request duration in seconds",
                config.buckets.clone()
            ),
            &["method", "path"],
        )
        .expect("metric can be created");

        let http_requests_in_flight = IntGauge::new(
            format!("{}http_requests_in_flight", prefix),
            "Number of HTTP requests currently being processed",
        )
        .expect("metric can be created");

        registry
            .register(Box::new(http_requests_total.clone()))
            .expect("metric can be registered");
        registry
            .register(Box::new(http_request_duration_seconds.clone()))
            .expect("metric can be registered");
        registry
            .register(Box::new(http_requests_in_flight.clone()))
            .expect("metric can be registered");

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            http_requests_in_flight,
        }
    }
}

/// Configuration for metrics.
#[derive(Clone)]
pub struct MetricsConfig {
    pub namespace: Option<String>,
    pub buckets: Vec<f64>,
    pub exclude_paths: Vec<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            namespace: None,
            // Default buckets: 1ms to 10s
            buckets: exponential_buckets(0.001, 2.0, 14).unwrap(),
            exclude_paths: vec![],
        }
    }
}

/// Initialize global metrics with the given config.
/// Returns the metrics instance (or existing one if already initialized).
pub fn init_metrics(config: &MetricsConfig) -> &'static Metrics {
    METRICS.get_or_init(|| Metrics::new(config))
}

/// Get the global metrics instance.
/// Panics if metrics haven't been initialized.
pub fn metrics() -> &'static Metrics {
    METRICS
        .get()
        .expect("Metrics not initialized. Call init_metrics() first.")
}

/// Encode all metrics to Prometheus text format.
pub fn encode_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = metrics().registry.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

/// Record an HTTP request.
pub fn record_request(method: &str, path: &str, status: u16, duration_secs: f64) {
    let m = metrics();
    m.http_requests_total
        .with_label_values(&[method, path, &status.to_string()])
        .inc();
    m.http_request_duration_seconds
        .with_label_values(&[method, path])
        .observe(duration_secs);
}

/// Increment in-flight requests counter.
pub fn inc_in_flight() {
    metrics().http_requests_in_flight.inc();
}

/// Decrement in-flight requests counter.
pub fn dec_in_flight() {
    metrics().http_requests_in_flight.dec();
}
