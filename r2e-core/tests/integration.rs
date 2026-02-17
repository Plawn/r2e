use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use r2e_core::builder::AppBuilder;
use r2e_core::plugins::{DevReload, ErrorHandling, Health, NormalizePath};
use r2e_core::request_id::RequestIdPlugin;
use r2e_core::secure_headers::SecureHeaders;
use tower::ServiceExt;

async fn send_get(router: axum::Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn send_get_with_header(
    router: axum::Router,
    path: &str,
    header_name: &str,
    header_value: &str,
) -> axum::http::Response<Body> {
    let req = Request::builder()
        .uri(path)
        .header(header_name, header_value)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap()
}

fn build_app() -> AppBuilder<()> {
    AppBuilder::new().with_state(())
}

// ── Health plugin ────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200_ok() {
    let router = build_app().with(Health).build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── ErrorHandling plugin ────────────────────────────────────────────────

#[tokio::test]
async fn error_handling_catches_panic() {
    use axum::routing::get;

    let app = AppBuilder::new()
        .with_state(())
        .register_routes(axum::Router::new().route(
            "/panic",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                "never"
            }),
        ))
        .with(ErrorHandling)
        .build();

    let (status, body) = send_get(app, "/panic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "Internal server error");
}

// ── NormalizePath plugin ────────────────────────────────────────────────

#[tokio::test]
async fn normalize_path_strips_trailing() {
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "/health/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── DevReload plugin ────────────────────────────────────────────────────

#[tokio::test]
async fn dev_reload_status() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "dev");
}

#[tokio::test]
async fn dev_reload_ping() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/ping").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["boot_time"].is_number());
    assert_eq!(json["status"], "ok");
}

// ── RequestIdPlugin ─────────────────────────────────────────────────────

#[tokio::test]
async fn request_id_generated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id");
    assert!(req_id.is_some(), "response should have X-Request-Id header");
    let val = req_id.unwrap().to_str().unwrap();
    assert!(!val.is_empty());
}

#[tokio::test]
async fn request_id_propagated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "x-request-id", "test-123").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id").unwrap().to_str().unwrap();
    assert_eq!(req_id, "test-123");
}

// ── SecureHeaders plugin ────────────────────────────────────────────────

#[tokio::test]
async fn secure_headers_in_response() {
    let router = build_app()
        .with(Health)
        .with(SecureHeaders::default())
        .build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let headers = resp.headers();
    assert_eq!(
        headers.get("x-content-type-options").unwrap().to_str().unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get("x-frame-options").unwrap().to_str().unwrap(),
        "DENY"
    );
    assert!(headers.get("strict-transport-security").is_some());
    assert_eq!(
        headers.get("x-xss-protection").unwrap().to_str().unwrap(),
        "0"
    );
    assert!(headers.get("referrer-policy").is_some());
}

// ── Advanced Health ─────────────────────────────────────────────────────

use r2e_core::health::{HealthIndicator, HealthStatus};

struct AlwaysUp;
impl HealthIndicator for AlwaysUp {
    fn name(&self) -> &str {
        "always-up"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Up }
    }
}

struct AlwaysDown;
impl HealthIndicator for AlwaysDown {
    fn name(&self) -> &str {
        "always-down"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Down("intentionally down".into()) }
    }
}

struct LivenessOnly;
impl HealthIndicator for LivenessOnly {
    fn name(&self) -> &str {
        "liveness-only"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Down("down but liveness-only".into()) }
    }
    fn affects_readiness(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn advanced_health_all_up() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

#[tokio::test]
async fn advanced_health_one_down() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).check(AlwaysDown).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "DOWN");
}

#[tokio::test]
async fn advanced_health_liveness_always_ok() {
    let router = build_app()
        .with(Health::builder().check(AlwaysDown).build())
        .build();
    let (status, _body) = send_get(router, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn advanced_health_readiness_filters() {
    // LivenessOnly check is down but doesn't affect readiness
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).check(LivenessOnly).build())
        .build();
    let (status, body) = send_get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}
