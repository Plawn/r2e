//! Layer-only mode: HTTP tracking without the `/metrics` endpoint.
//!
//! `Prometheus::layer_only()` (and its config spelling
//! `prometheus.expose_endpoint: false`) keeps the tracking layer and the
//! registry but leaves the exposition to the application. All tests in this
//! binary share the process-level metrics singleton, so assertions are scoped
//! to the route labels each test uniquely produces.

use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::AppBuilder;
use r2e_prometheus::{encode_metrics, Prometheus};
use tower::ServiceExt;

async fn status_of(router: Router, path: &str) -> StatusCode {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    router.oneshot(req).await.unwrap().status()
}

#[r2e_core::test]
async fn layer_only_tracks_requests_but_mounts_no_endpoint() {
    let app = AppBuilder::new()
        .plugin(Prometheus::layer_only())
        .build_state()
        .await
        .register_routes(Router::new().route("/only-layer", get(|| async { "ok" })));

    let router = app.build();

    assert_eq!(
        status_of(router.clone(), "/only-layer").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(router, "/metrics").await,
        StatusCode::NOT_FOUND,
        "layer-only mode must not mount the metrics endpoint"
    );

    // … but the request WAS tracked: the app can still scrape the registry
    // itself (its own route, a push gateway, a sidecar).
    let output = encode_metrics();
    assert!(
        output.contains("path=\"/only-layer\""),
        "layer-only mode still records requests; got:\n{output}"
    );
}

#[r2e_core::test]
async fn expose_endpoint_false_in_config_is_the_same_switch() {
    let config = R2eConfig::from_yaml_str("prometheus:\n  expose_endpoint: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await
        .register_routes(Router::new().route("/from-config", get(|| async { "ok" })));

    let router = app.build();

    assert_eq!(
        status_of(router.clone(), "/from-config").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(router, "/metrics").await,
        StatusCode::NOT_FOUND,
        "`prometheus.expose_endpoint: false` must not mount the endpoint, \
         even with an explicit endpoint path"
    );
    assert!(encode_metrics().contains("path=\"/from-config\""));
}

#[r2e_core::test]
async fn builder_without_endpoint_wins_over_expose_endpoint_true_in_config() {
    // Same precedence rule as every other knob: builder setting > file config.
    let config = R2eConfig::from_yaml_str("prometheus:\n  expose_endpoint: true\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::builder().without_endpoint().build())
        .build_state()
        .await;

    assert_eq!(
        status_of(app.build(), "/metrics").await,
        StatusCode::NOT_FOUND
    );
}

#[r2e_core::test]
async fn expose_endpoint_true_keeps_the_default_behavior() {
    let config = R2eConfig::from_yaml_str("prometheus:\n  expose_endpoint: true\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    assert_eq!(status_of(app.build(), "/metrics").await, StatusCode::OK);
}
