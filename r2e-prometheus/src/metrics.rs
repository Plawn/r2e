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

/// Returns `true` if metrics have been initialized.
pub fn is_initialized() -> bool {
    METRICS.get().is_some()
}

/// Get the global metrics instance, lazily initializing with defaults.
///
/// Normally `init_metrics` runs first (from the plugin's `configure` step with
/// the merged builder/file config). Lazy default-init exists for the disabled
/// plugin case (`prometheus.enabled = false` skips `configure`, but the
/// `PrometheusRegistry` bean stays injectable and must not panic): callers get
/// a real registry that simply is not exported at the metrics endpoint.
pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(|| Metrics::new(&MetricsConfig::default()))
}

/// Get the global prometheus Registry (lazily default-initialized; see
/// [`metrics`]).
pub fn registry() -> &'static Registry {
    &metrics().registry
}

/// Encode all metrics to Prometheus text format.
pub fn encode_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = metrics().registry.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

/// Render the status-code label without allocating.
///
/// `with_label_values` wants `&str`, and the naive `status.to_string()` is a
/// heap allocation on every single request — the only one left on this path,
/// since `with_label_values` itself just hashes the values and looks the
/// metric up. The value is drawn from a tiny bounded set, so it renders into
/// a stack buffer instead.
fn status_label(status: u16, buf: &mut [u8; 5]) -> &str {
    use std::io::Write as _;
    let cap = buf.len();
    let written = {
        let mut cursor = &mut buf[..];
        // Infallible: a `u16` is at most 5 digits.
        let _ = write!(cursor, "{status}");
        cap - cursor.len()
    };
    // `write!` of a `u16` emits ASCII digits only, so this never fails.
    std::str::from_utf8(&buf[..written]).unwrap_or("")
}

/// Record an HTTP request.
pub fn record_request(method: &str, path: &str, status: u16, duration_secs: f64) {
    let m = metrics();
    let mut status_buf = [0u8; 5];
    m.http_requests_total
        .with_label_values(&[method, path, status_label(status, &mut status_buf)])
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
