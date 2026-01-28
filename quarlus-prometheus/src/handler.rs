use crate::metrics::encode_metrics;

/// Handler for the /metrics endpoint.
/// Returns metrics in Prometheus text format.
pub async fn metrics_handler() -> ([(&'static str, &'static str); 1], String) {
    let body = encode_metrics();
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}
