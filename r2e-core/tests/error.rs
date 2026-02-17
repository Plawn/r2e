use r2e_core::AppError;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;

async fn error_parts(err: AppError) -> (StatusCode, serde_json::Value) {
    let resp = err.into_response();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[tokio::test]
async fn app_error_not_found_status() {
    let (status, body) = error_parts(AppError::NotFound("resource missing".into())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "resource missing");
}

#[tokio::test]
async fn app_error_bad_request_status() {
    let (status, body) = error_parts(AppError::BadRequest("invalid input".into())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid input");
}

#[tokio::test]
async fn app_error_unauthorized_status() {
    let (status, body) = error_parts(AppError::Unauthorized("no token".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "no token");
}

#[tokio::test]
async fn app_error_forbidden_status() {
    let (status, body) = error_parts(AppError::Forbidden("access denied".into())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "access denied");
}

#[tokio::test]
async fn app_error_internal_status() {
    let (status, body) = error_parts(AppError::Internal("server broke".into())).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "server broke");
}

#[tokio::test]
async fn app_error_custom_status_and_body() {
    let custom_body = serde_json::json!({"detail": "teapot"});
    let (status, body) = error_parts(AppError::Custom {
        status: StatusCode::IM_A_TEAPOT,
        body: custom_body.clone(),
    })
    .await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);
    assert_eq!(body, custom_body);
}

#[test]
fn app_error_display_formatting() {
    assert_eq!(
        AppError::NotFound("x".into()).to_string(),
        "Not Found: x"
    );
    assert_eq!(
        AppError::Unauthorized("y".into()).to_string(),
        "Unauthorized: y"
    );
    assert_eq!(
        AppError::Forbidden("z".into()).to_string(),
        "Forbidden: z"
    );
    assert_eq!(
        AppError::BadRequest("w".into()).to_string(),
        "Bad Request: w"
    );
    assert_eq!(
        AppError::Internal("v".into()).to_string(),
        "Internal Error: v"
    );
}

#[test]
fn app_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let app_err: AppError = io_err.into();
    match app_err {
        AppError::Internal(msg) => assert!(msg.contains("file missing")),
        other => panic!("expected Internal, got {other}"),
    }
}
