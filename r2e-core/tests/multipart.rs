#![cfg(feature = "multipart")]

use http_body_util::BodyExt;
use r2e_core::http::{IntoResponse, StatusCode};
use r2e_core::multipart::MultipartError;

async fn status_of(err: MultipartError) -> StatusCode {
    err.into_response().status()
}

#[r2e_core::test]
async fn field_too_large_maps_to_413() {
    let err = MultipartError::FieldTooLarge {
        field: "avatar".into(),
        limit: 1024,
    };
    assert_eq!(status_of(err).await, StatusCode::PAYLOAD_TOO_LARGE);
}

#[r2e_core::test]
async fn payload_too_large_maps_to_413() {
    let err = MultipartError::PayloadTooLarge { limit: 1024 };
    assert_eq!(status_of(err).await, StatusCode::PAYLOAD_TOO_LARGE);
}

#[r2e_core::test]
async fn missing_field_still_maps_to_400() {
    let err = MultipartError::MissingField("name".into());
    assert_eq!(status_of(err).await, StatusCode::BAD_REQUEST);
}

#[r2e_core::test]
async fn field_too_large_body_contains_limit() {
    let err = MultipartError::FieldTooLarge {
        field: "avatar".into(),
        limit: 4096,
    };
    let resp = err.into_response();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let msg = json["error"].as_str().unwrap();
    assert!(msg.contains("avatar"), "error should name the field: {msg}");
    assert!(msg.contains("4096"), "error should include the limit: {msg}");
}

#[r2e_core::test]
async fn default_limits_are_sensible() {
    let defaults = r2e_core::multipart::MultipartLimits::DEFAULT;
    assert_eq!(defaults.per_field, 10 * 1024 * 1024);
    assert_eq!(defaults.total, 100 * 1024 * 1024);
    assert!(defaults.per_field <= defaults.total);
}
