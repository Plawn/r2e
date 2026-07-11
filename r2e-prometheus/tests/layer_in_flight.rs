//! In-flight gauge balance when a request future is dropped mid-flight
//! (client disconnect). Kept in its own test binary because the assertion
//! reads the process-global gauge and must not race requests from other tests
//! sharing the metrics singleton.

use r2e_prometheus::{init_metrics, metrics, MetricsConfig, PrometheusLayer};

use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router};
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn dropped_request_future_decrements_in_flight() {
    init_metrics(&MetricsConfig::default());
    let router = Router::new()
        .route(
            "/hang",
            get(|| async {
                std::future::pending::<()>().await;
                "unreachable"
            }),
        )
        .layer(PrometheusLayer::new(MetricsConfig::default()));

    let req = Request::builder().uri("/hang").body(Body::empty()).unwrap();
    // Poll the request into flight, then drop it — a client disconnect.
    let cancelled = tokio::time::timeout(Duration::from_millis(50), router.oneshot(req)).await;
    assert!(cancelled.is_err(), "hanging request must not complete");

    assert_eq!(
        metrics().http_requests_in_flight.get(),
        0,
        "a dropped in-flight request must still decrement the gauge"
    );
}
