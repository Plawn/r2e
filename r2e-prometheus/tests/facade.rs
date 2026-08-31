//! `metrics`-facade backend: R2E's HTTP metrics land in the application's own
//! recorder, and this crate's `prometheus` registry is never touched.
//!
//! Own test target on purpose: `metrics::set_global_recorder` (through
//! `DebuggingRecorder::install`) is a one-shot process-global write, and the
//! `prometheus` `METRICS` singleton assertion below can only be meaningful in a
//! process where nothing else booted the `Prometheus` plugin.
#![cfg(feature = "metrics-facade")]

use std::collections::HashMap;

use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::AppBuilder;
use r2e_prometheus::MetricsFacade;
use tower::ServiceExt;

/// `(metric name, sorted "k=v" labels)` → value, for the whole snapshot.
fn snapshot(snapshotter: &Snapshotter) -> Vec<(String, Vec<String>, DebugValue)> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .map(|(composite, _unit, _desc, value)| {
            let key = composite.key();
            let mut labels: Vec<String> = key
                .labels()
                .map(|l| format!("{}={}", l.key(), l.value()))
                .collect();
            labels.sort();
            (key.name().to_string(), labels, value)
        })
        .collect()
}

/// The one series named `name` whose labels start with `label_prefix`.
fn find<'a>(
    snap: &'a [(String, Vec<String>, DebugValue)],
    name: &str,
    label_prefix: &[&str],
) -> &'a (String, Vec<String>, DebugValue) {
    let mut matches = snap.iter().filter(|(n, labels, _)| {
        n == name
            && label_prefix.len() <= labels.len()
            && label_prefix.iter().zip(labels).all(|(a, b)| a == b)
    });
    let found = matches
        .next()
        .unwrap_or_else(|| panic!("no `{name}` series with labels {label_prefix:?} in {snap:?}"));
    assert!(
        matches.next().is_none(),
        "`{name}` with labels {label_prefix:?} is ambiguous in {snap:?}"
    );
    found
}

#[r2e_core::test]
async fn facade_plugin_records_http_metrics_into_the_apps_recorder() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder
        .install()
        .expect("the app installs its own global recorder");

    let app = AppBuilder::new()
        .plugin(MetricsFacade::builder().exclude_path("/health").build())
        .build_state()
        .await
        .register_routes(
            Router::new()
                .route("/users/{id}", get(|| async { "user" }))
                .route("/health", get(|| async { "ok" })),
        );

    let router = app.build();

    // The plugin owns no endpoint: exposition is the application's business.
    let req = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "the facade plugin must not mount a metrics endpoint"
    );

    for uri in ["/users/1", "/users/2", "/health"] {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let status = router.clone().oneshot(req).await.unwrap().status();
        assert_eq!(status, StatusCode::OK);
    }

    let snap = snapshot(&snapshotter);
    let by_name: HashMap<&str, ()> = snap.iter().map(|(n, _, _)| (n.as_str(), ())).collect();
    assert!(
        by_name.contains_key("http_requests_total")
            && by_name.contains_key("http_request_duration_seconds")
            && by_name.contains_key("http_requests_in_flight"),
        "expected R2E's three HTTP metrics through the facade, got: {:?}",
        snap.iter().map(|(n, _, _)| n).collect::<Vec<_>>()
    );

    // Counter: bounded labels (route template, not the concrete URL), and the
    // two `/users/{id}` requests share one series.
    let (_, labels, value) = find(
        &snap,
        "http_requests_total",
        &["method=GET", "path=/users/{id}"],
    );
    assert_eq!(
        labels,
        &vec![
            "method=GET".to_string(),
            "path=/users/{id}".to_string(),
            "status=200".to_string()
        ]
    );
    assert!(
        matches!(value, DebugValue::Counter(2)),
        "both /users/N requests collapse into one series: {value:?}"
    );

    // Unmatched requests (the 404 on /metrics above) collapse into the sentinel
    // instead of minting a series per URL — same bounding as the prometheus
    // backend, since both go through `r2e_core::http::labels`.
    find(
        &snap,
        "http_requests_total",
        &["method=GET", "path=unmatched", "status=404"],
    );

    // Histogram: method + path only, one observation per request.
    let (_, labels, value) = find(
        &snap,
        "http_request_duration_seconds",
        &["method=GET", "path=/users/{id}"],
    );
    assert_eq!(
        labels,
        &vec!["method=GET".to_string(), "path=/users/{id}".to_string()]
    );
    match value {
        DebugValue::Histogram(samples) => assert_eq!(samples.len(), 2),
        other => panic!("expected a histogram, got {other:?}"),
    }

    // In-flight gauge: unlabeled, balanced back to zero once every request is
    // done (the RAII guard runs on completion AND on cancellation).
    let (_, labels, value) = find(&snap, "http_requests_in_flight", &[]);
    assert!(labels.is_empty(), "the in-flight gauge carries no labels");
    assert!(
        matches!(value, DebugValue::Gauge(g) if g.into_inner() == 0.0),
        "in-flight gauge must be balanced: {value:?}"
    );

    // `/health` was excluded, so it minted no series at all.
    assert!(
        !snap
            .iter()
            .any(|(_, labels, _)| labels.iter().any(|l| l == "path=/health")),
        "excluded paths must not be recorded: {snap:?}"
    );

    // And the whole point of this backend: R2E's own `prometheus` registry was
    // never installed — one metrics stack in the process, the app's.
    assert!(
        !r2e_prometheus::is_initialized(),
        "the facade backend must not install this crate's prometheus registry"
    );
}
