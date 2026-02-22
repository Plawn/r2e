use r2e_security::error::SecurityError;

use http_body_util::BodyExt;
use r2e_core::http::response::IntoResponse;
use r2e_core::http::StatusCode;

async fn error_parts(err: SecurityError) -> (StatusCode, serde_json::Value) {
    let resp = err.into_response();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[tokio::test]
async fn missing_auth_header_401() {
    let (status, body) = error_parts(SecurityError::MissingAuthHeader).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn invalid_auth_scheme_401() {
    let (status, body) = error_parts(SecurityError::InvalidAuthScheme).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn invalid_token_401() {
    let (status, body) = error_parts(SecurityError::InvalidToken("bad sig".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn token_expired_401() {
    let (status, body) = error_parts(SecurityError::TokenExpired).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn unknown_key_id_401() {
    let (status, body) = error_parts(SecurityError::UnknownKeyId("kid-123".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn jwks_fetch_error_401() {
    let (status, body) =
        error_parts(SecurityError::JwksFetchError("timeout".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[tokio::test]
async fn validation_failed_401() {
    let (status, body) =
        error_parts(SecurityError::ValidationFailed("bad issuer".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
}

#[test]
fn display_formatting() {
    assert_eq!(
        SecurityError::MissingAuthHeader.to_string(),
        "Missing Authorization header"
    );
    assert_eq!(
        SecurityError::InvalidAuthScheme.to_string(),
        "Invalid authorization scheme"
    );
    assert_eq!(
        SecurityError::InvalidToken("x".into()).to_string(),
        "Invalid token: x"
    );
    assert_eq!(SecurityError::TokenExpired.to_string(), "Token expired");
    assert_eq!(
        SecurityError::UnknownKeyId("k".into()).to_string(),
        "Unknown signing key: k"
    );
    assert_eq!(
        SecurityError::JwksFetchError("e".into()).to_string(),
        "JWKS fetch error: e"
    );
    assert_eq!(
        SecurityError::ValidationFailed("v".into()).to_string(),
        "Token validation failed: v"
    );
}

#[test]
fn into_app_error() {
    let sec_err = SecurityError::InvalidToken("bad".into());
    let app_err: r2e_core::HttpError = sec_err.into();
    match app_err {
        r2e_core::HttpError::Unauthorized(msg) => {
            assert_eq!(msg, "Unauthorized");
        }
        other => panic!("expected Unauthorized, got {other}"),
    }
}

#[tokio::test]
async fn json_body_format() {
    let (_, body) = error_parts(SecurityError::TokenExpired).await;
    // Verify the body is a JSON object with an "error" key
    assert!(body.is_object());
    assert!(body.get("error").is_some());
    assert!(body.get("error").unwrap().is_string());
}
