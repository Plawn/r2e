use r2e_core::request_id::RequestId;
use axum::response::IntoResponse;
use http_body_util::BodyExt;

#[test]
fn request_id_display() {
    let id = RequestId("abc-123".into());
    assert_eq!(id.to_string(), "abc-123");
}

#[tokio::test]
async fn request_id_into_response() {
    let id = RequestId("test-id".into());
    let resp = id.into_response();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"test-id");
}
