use r2e_core::managed::{ManagedErr, ManagedError};
use r2e_core::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;

#[tokio::test]
async fn managed_error_into_response() {
    let err = ManagedError(AppError::NotFound("gone".into()));
    let resp: Response = err.into();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "gone");
}

#[derive(Debug)]
struct TestError(String);

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl IntoResponse for TestError {
    fn into_response(self) -> Response {
        (StatusCode::CONFLICT, self.0).into_response()
    }
}

#[tokio::test]
async fn managed_err_wraps_custom_error() {
    let err = ManagedErr(TestError("conflict!".into()));
    let resp: Response = err.into();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8_lossy(&body), "conflict!");
}

#[test]
fn managed_err_display_delegates() {
    let err = ManagedErr(TestError("hello".into()));
    assert_eq!(err.to_string(), "hello");
    assert_eq!(format!("{:?}", err), "ManagedErr(TestError(\"hello\"))");
}
