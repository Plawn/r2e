//! Path-label cardinality of the HTTP metrics layer.
//!
//! The `path` label must stay bounded under arbitrary-path traffic: matched
//! routes are labeled with their route template (`/users/{id}`), and every
//! unmatched request (404s, fallbacks) collapses into the single
//! `UNMATCHED_PATH_LABEL` sentinel.

use r2e_prometheus::{init_metrics, registry, MetricsConfig, PrometheusLayer, UNMATCHED_PATH_LABEL};

use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router, StatusCode};
use std::collections::HashSet;
use tower::ServiceExt;

// NOTE: All tests in this binary share the process-level metrics singleton,
// so assertions are scoped to the label values each test uniquely produces.

fn test_router(config: MetricsConfig) -> Router {
    init_metrics(&MetricsConfig::default());
    Router::new()
        .route("/users/{id}", get(|| async { "user" }))
        .route("/health", get(|| async { "ok" }))
        .layer(PrometheusLayer::new(config))
}

async fn send(router: &Router, path: &str) -> StatusCode {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    router.clone().oneshot(req).await.unwrap().status()
}

/// All `path` label values currently present on `http_requests_total`.
fn recorded_paths() -> HashSet<String> {
    registry()
        .gather()
        .iter()
        .find(|f| f.name() == "http_requests_total")
        .map(|family| {
            family
                .get_metric()
                .iter()
                .flat_map(|m| m.get_label())
                .filter(|l| l.name() == "path")
                .map(|l| l.value().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn junk_404_paths_do_not_create_one_series_each() {
    let router = test_router(MetricsConfig::default());

    let junk = [
        "/wp-login.php",
        "/.env",
        "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php",
        "/admin/config.php",
        "/cgi-bin/luci",
        "/scan-a",
        "/scan-b",
        "/scan-c",
    ];
    for path in junk {
        assert_eq!(send(&router, path).await, StatusCode::NOT_FOUND);
    }

    let paths = recorded_paths();
    assert!(
        paths.contains(UNMATCHED_PATH_LABEL),
        "unmatched requests should be recorded under the sentinel label"
    );
    for path in junk {
        assert!(
            !paths.contains(path),
            "raw junk path {path:?} must not become a label value"
        );
    }
}

#[tokio::test]
async fn matched_routes_are_labeled_with_the_route_template() {
    let router = test_router(MetricsConfig::default());

    for path in ["/users/1", "/users/2", "/users/de5c8bd2-aaaa-bbbb-cccc-1234567890ab"] {
        assert_eq!(send(&router, path).await, StatusCode::OK);
    }

    let paths = recorded_paths();
    assert!(
        paths.contains("/users/{id}"),
        "matched requests should be labeled with the route template"
    );
    assert!(
        !paths.contains("/users/1"),
        "raw request paths must not become label values"
    );
}

#[tokio::test]
async fn excluded_paths_are_not_recorded() {
    let config = MetricsConfig {
        exclude_paths: vec!["/health".to_string()],
        ..MetricsConfig::default()
    };
    let router = test_router(config);

    assert_eq!(send(&router, "/health").await, StatusCode::OK);

    assert!(
        !recorded_paths().contains("/health"),
        "excluded paths must not be recorded at all"
    );
}
