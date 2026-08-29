//! The two tower layers R2E puts in front of every route: the Prometheus
//! metrics layer and the OpenTelemetry trace layer.
//!
//! Both are cloned once per request by the HTTP backend — `Route::oneshot_inner`
//! does `self.0.clone().oneshot(req)` on the fully layered, boxed service — so
//! any owned `String`/`Vec` field of theirs is a per-request deep copy. That is
//! the exact regression this ticket started from.

use r2e::http::routing::get;
use r2e::http::{Body, Request, Router, StatusCode};
use r2e::r2e_observability::middleware::OtelTraceLayer;
use r2e::r2e_prometheus::{MetricsConfig, PrometheusLayer};
use tower::ServiceExt;

use crate::counter::{assert_config_size_invariant, runtime, steady_state, Alloc};

const ITERATIONS: u64 = 200;

fn request() -> Request<Body> {
    Request::builder()
        .uri("/plain")
        .body(Body::empty())
        .expect("request")
}

/// One route behind the layer stack under test. `.layer()` on the router puts
/// the layers inside the per-route boxed service, which is what the backend
/// clones per request.
fn router(prometheus: MetricsConfig, capture_headers: Vec<String>) -> Router {
    Router::new()
        .route("/plain", get(|| async { "ok" }))
        .layer(OtelTraceLayer::new(capture_headers))
        .layer(PrometheusLayer::new(prometheus))
}

fn drive(rt: &r2e::rt::Runtime, router: &Router) -> Alloc {
    steady_state(ITERATIONS, || {
        let response = rt
            .block_on(router.clone().oneshot(request()))
            .expect("infallible router");
        assert_eq!(response.status(), StatusCode::OK);
    })
}

fn many_paths(n: usize) -> Vec<String> {
    // Long, distinct entries: a deep clone costs one allocation AND ~64 bytes
    // each, so both halves of the invariant have something to catch.
    (0..n)
        .map(|i| format!("/excluded/{i:0>52}"))
        .collect()
}

/// `MetricsConfig.exclude_paths` (and `buckets`) must not be re-allocated per
/// request. Before `75495d5`, `PrometheusService` held `MetricsConfig` by value
/// and its derived `Clone` deep-copied the whole `Vec<String>` on every hit.
#[test]
fn prometheus_config_is_shared_not_copied_per_request() {
    let rt = runtime();

    let small = MetricsConfig {
        namespace: Some("r2e".into()),
        exclude_paths: vec!["/health".into()],
        ..MetricsConfig::default()
    };
    let large = MetricsConfig {
        namespace: Some("r2e".into()),
        exclude_paths: many_paths(64),
        ..MetricsConfig::default()
    };

    let small = drive(&rt, &router(small, vec!["x-request-id".into()]));
    let large = drive(&rt, &router(large, vec!["x-request-id".into()]));

    // A deep clone of the large config would cost >= 64 allocations and >= 4 KiB
    // per request; the slack is two orders of magnitude below that.
    assert_config_size_invariant(
        "PrometheusService / MetricsConfig",
        small,
        large,
        4,
        512,
    );
}

/// `OtelTraceLayer.capture_headers` must not be re-allocated per request. It is
/// an `Arc<[String]>` built once in `OtelTraceLayer::new`; a `Vec<String>` there
/// would deep-copy every configured header name on every request.
#[test]
fn observability_capture_headers_are_shared_not_copied_per_request() {
    let rt = runtime();

    let base = || MetricsConfig {
        exclude_paths: vec!["/health".into()],
        ..MetricsConfig::default()
    };

    let small = drive(&rt, &router(base(), vec!["x-request-id".into()]));
    let large = drive(&rt, &router(base(), many_paths(64)));

    assert_config_size_invariant(
        "OtelTraceService / capture_headers",
        small,
        large,
        4,
        512,
    );
}

/// The absolute figure, for visibility: what one request through the full
/// wrapper stack costs on this machine.
///
/// The bound is deliberately generous — it is a canary for an order-of-magnitude
/// regression (a config deep-copy, a per-request `Vec` rebuild), not a
/// micro-budget. To re-baseline, run
/// `cargo test -p example-app --test hotpath -- --nocapture` and read the
/// `[hotpath] composed stack` line.
#[test]
fn composed_stack_budget() {
    let rt = runtime();
    let cost = drive(
        &rt,
        &router(
            MetricsConfig {
                namespace: Some("r2e".into()),
                exclude_paths: vec!["/health".into(), "/metrics".into()],
                ..MetricsConfig::default()
            },
            vec!["x-request-id".into()],
        ),
    );

    eprintln!("[hotpath] composed stack (prometheus + otel + route): {cost} per request");

    const MAX_ALLOCATIONS: u64 = 120;
    const MAX_BYTES: u64 = 16_384;
    assert!(
        cost.count <= MAX_ALLOCATIONS && cost.bytes <= MAX_BYTES,
        "per-request cost through the framework layer stack regressed: {cost} \
         (budget: {MAX_ALLOCATIONS} allocations / {MAX_BYTES} bytes). See \
         docs/claude/hot-path-clone-audit.md before raising the budget.",
    );
}
