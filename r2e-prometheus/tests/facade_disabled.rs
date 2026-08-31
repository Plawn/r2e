//! `metrics.enabled = false` makes the facade plugin inert: no tracking layer.
//!
//! Own target: it installs a process-global `metrics` recorder, which is a
//! one-shot write per process.
#![cfg(feature = "metrics-facade")]

use metrics_util::debugging::DebuggingRecorder;
use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::AppBuilder;
use r2e_prometheus::MetricsFacade;
use tower::ServiceExt;

#[r2e_core::test]
async fn disabled_facade_plugin_installs_no_tracking_layer() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install().expect("app-owned global recorder");

    let config = R2eConfig::from_yaml_str("metrics:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(MetricsFacade::new())
        .build_state()
        .await
        .register_routes(Router::new().route("/ping", get(|| async { "ok" })));

    let router = app.build();
    let req = Request::builder().uri("/ping").body(Body::empty()).unwrap();
    assert_eq!(router.oneshot(req).await.unwrap().status(), StatusCode::OK);

    assert!(
        snapshotter.snapshot().into_vec().is_empty(),
        "a disabled plugin must record nothing at all"
    );
}
