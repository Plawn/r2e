use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use r2e_core::prelude::*;

async fn error_parts(err: impl IntoResponse) -> (StatusCode, serde_json::Value) {
    let resp = err.into_response();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

// ── Basic: explicit message with {0} interpolation ──────────────────────

#[derive(Debug, ApiError)]
pub enum SimpleError {
    #[error(status = NOT_FOUND, message = "User not found: {0}")]
    NotFound(String),
}

#[tokio::test]
async fn explicit_message_with_interpolation() {
    let err = SimpleError::NotFound("alice".into());
    assert_eq!(err.to_string(), "User not found: alice");

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "User not found: alice");
}

// ── No-message String field → uses field value ──────────────────────────

#[derive(Debug, ApiError)]
pub enum InferredError {
    #[error(status = BAD_REQUEST)]
    Validation(String),
}

#[tokio::test]
async fn no_message_string_field_uses_value() {
    let err = InferredError::Validation("name is required".into());
    assert_eq!(err.to_string(), "name is required");

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "name is required");
}

// ── Unit variant → humanized name ───────────────────────────────────────

#[derive(Debug, ApiError)]
pub enum UnitError {
    #[error(status = CONFLICT)]
    AlreadyExists,
}

#[tokio::test]
async fn unit_variant_humanized_name() {
    let err = UnitError::AlreadyExists;
    assert_eq!(err.to_string(), "Already exists");

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "Already exists");
}

// ── #[from] → From impl + source() ─────────────────────────────────────

#[derive(Debug, ApiError)]
pub enum FromError {
    #[error(status = INTERNAL_SERVER_ERROR, message = "IO error")]
    Io(#[from] std::io::Error),
}

#[test]
fn from_impl_works() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let err: FromError = io_err.into();
    match &err {
        FromError::Io(_) => {}
    }
    assert_eq!(err.to_string(), "IO error");

    // source() returns the inner error
    let source = std::error::Error::source(&err).unwrap();
    assert!(source.to_string().contains("file missing"));
}

#[tokio::test]
async fn from_variant_response() {
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
    let err: FromError = io_err.into();
    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "IO error");
}

// ── #[from] without explicit message → uses source.to_string() ─────────

#[derive(Debug, ApiError)]
pub enum FromInferError {
    #[error(status = INTERNAL_SERVER_ERROR)]
    Io(#[from] std::io::Error),
}

#[tokio::test]
async fn from_inferred_message_uses_source() {
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
    let err: FromInferError = io_err.into();
    assert_eq!(err.to_string(), "disk full");

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "disk full");
}

// ── #[error(transparent)] → delegates to inner IntoResponse ─────────────

#[derive(Debug, ApiError)]
pub enum TransparentError {
    #[error(transparent)]
    Inner(#[from] HttpError),
}

#[tokio::test]
async fn transparent_delegates_into_response() {
    let inner = HttpError::Forbidden("no access".into());
    let err: TransparentError = inner.into();

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "no access");
}

#[test]
fn transparent_display_delegates() {
    let inner = HttpError::NotFound("gone".into());
    let err: TransparentError = inner.into();
    assert_eq!(err.to_string(), "Not Found: gone");
}

// ── Numeric status code ─────────────────────────────────────────────────

#[derive(Debug, ApiError)]
pub enum NumericStatusError {
    #[error(status = 429, message = "Too many requests")]
    RateLimited,
}

#[tokio::test]
async fn numeric_status_code() {
    let err = NumericStatusError::RateLimited;
    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "Too many requests");
}

// ── Named fields with {field} interpolation ─────────────────────────────

#[derive(Debug, ApiError)]
pub enum NamedFieldsError {
    #[error(status = BAD_REQUEST, message = "Field {field} is invalid: {reason}")]
    InvalidField { field: String, reason: String },
}

#[tokio::test]
async fn named_field_interpolation() {
    let err = NamedFieldsError::InvalidField {
        field: "email".into(),
        reason: "must contain @".into(),
    };
    assert_eq!(err.to_string(), "Field email is invalid: must contain @");

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Field email is invalid: must contain @");
}

// ── Mixed enum: all variant kinds together ──────────────────────────────

#[derive(Debug, ApiError)]
pub enum MixedError {
    #[error(status = NOT_FOUND, message = "Resource {0} not found")]
    NotFound(String),

    #[error(status = INTERNAL_SERVER_ERROR)]
    Io(#[from] std::io::Error),

    #[error(status = BAD_REQUEST)]
    Validation(String),

    #[error(status = CONFLICT)]
    AlreadyExists,
}

#[tokio::test]
async fn mixed_enum_variants() {
    // Explicit message
    let (s, b) = error_parts(MixedError::NotFound("item-42".into())).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["error"], "Resource item-42 not found");

    // From impl
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "broken pipe");
    let err: MixedError = io_err.into();
    let (s, b) = error_parts(err).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(b["error"], "broken pipe");

    // Inferred string
    let (s, b) = error_parts(MixedError::Validation("bad input".into())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"], "bad input");

    // Unit
    let (s, b) = error_parts(MixedError::AlreadyExists).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["error"], "Already exists");
}

// ── Error::source() returns None for non-from variants ──────────────────

#[test]
fn source_none_for_non_from_variants() {
    let err = MixedError::NotFound("x".into());
    assert!(std::error::Error::source(&err).is_none());

    let err = MixedError::Validation("y".into());
    assert!(std::error::Error::source(&err).is_none());

    let err = MixedError::AlreadyExists;
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn source_some_for_from_variant() {
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test");
    let err: MixedError = io_err.into();
    assert!(std::error::Error::source(&err).is_some());
}
