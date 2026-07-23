use http_body_util::BodyExt;
use r2e_core::http::IntoResponse;
use r2e_core::request_id::RequestId;

#[test]
fn request_id_display() {
    let id = RequestId("abc-123".into());
    assert_eq!(id.to_string(), "abc-123");
}

#[r2e_core::test]
async fn request_id_into_response() {
    let id = RequestId("test-id".into());
    let resp = id.into_response();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"test-id");
}

// ── Plugin-level: header generation / propagation ─────────────────────────

use r2e_core::builder::AppBuilder;
use r2e_core::http::StatusCode;
use r2e_core::plugins::Health;
use r2e_core::request_id::RequestIdPlugin;

use crate::support::raw_get_with;

fn build_app() -> AppBuilder<()> {
    AppBuilder::new().with_state(())
}

#[r2e_core::test]
async fn request_id_generated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = raw_get_with(router, "/health", &[("accept", "*/*")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id");
    assert!(req_id.is_some(), "response should have X-Request-Id header");
    let val = req_id.unwrap().to_str().unwrap();
    assert!(!val.is_empty());
}

#[r2e_core::test]
async fn request_id_propagated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = raw_get_with(router, "/health", &[("x-request-id", "test-123")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(req_id, "test-123");
}
