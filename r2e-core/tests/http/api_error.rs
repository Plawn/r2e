use http_body_util::BodyExt;
use r2e_core::http::{IntoResponse, StatusCode};
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

// ── #[error(rejection)]: the enum becomes a projection envelope ─────────

#[derive(Debug, ApiError)]
pub enum Envelope {
    #[error(status = CONFLICT)]
    Duplicate,
    #[error(status = 418)]
    Teapot,
    #[error(rejection)]
    Rejected(Rejection),
}

#[r2e_core::test]
async fn rejection_variant_wraps_the_framework_failure() {
    let err: Envelope = Rejection::not_found("no such thing").into();
    assert!(matches!(err, Envelope::Rejected(_)));
    assert_eq!(err.to_string(), "no such thing");
    assert!(std::error::Error::source(&err).is_some());

    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no such thing");
}

#[r2e_core::test]
async fn rejection_variant_keeps_hub_headers_and_carried_status() {
    use r2e_core::http::header::{HeaderValue, RETRY_AFTER};
    let r = Rejection::with_status(
        RejectionKind::RateLimited,
        StatusCode::TOO_MANY_REQUESTS,
        "Rate limit exceeded",
    )
    .header(RETRY_AFTER, HeaderValue::from_static("3"));
    let resp = Envelope::from(r).into_response();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers()[RETRY_AFTER], "3");

    let resp = Rejection::with_status(RejectionKind::BadRequest, StatusCode::IM_A_TEAPOT, "t")
        .project::<Envelope>();
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
}

#[test]
fn rejection_variant_derives_error_schema() {
    assert_eq!(
        Envelope::status_of(RejectionKind::Validation),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        Envelope::status_of(RejectionKind::InvalidBody),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(Envelope::body_schema(), HttpError::body_schema());
    assert_eq!(
        Envelope::body_schema_for(RejectionKind::Validation),
        HttpError::body_schema_for(RejectionKind::Validation)
    );
    assert!(Envelope::opaque_passthrough());
    assert_eq!(
        Envelope::extra_statuses(),
        vec![
            (StatusCode::CONFLICT, "Duplicate"),
            (StatusCode::IM_A_TEAPOT, "Teapot"),
        ]
    );
}

#[derive(Debug, ApiError)]
pub enum NamedEnvelope {
    #[error(status = BAD_GATEWAY, message = "upstream {service} is down")]
    Upstream { service: String },
    #[error(status = BAD_GATEWAY)]
    UpstreamToo,
    #[error(rejection)]
    Rejected { inner: Rejection },
}

#[r2e_core::test]
async fn rejection_variant_with_a_named_field() {
    let err = NamedEnvelope::from(Rejection::forbidden("nope"));
    assert!(matches!(err, NamedEnvelope::Rejected { .. }));
    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "nope");
    // Duplicate statuses collapse.
    assert_eq!(
        NamedEnvelope::extra_statuses(),
        vec![(StatusCode::BAD_GATEWAY, "Upstream")]
    );
}

// `#[error(transparent)]` over a `Rejection` field is the same thing.
#[derive(Debug, ApiError)]
pub enum TransparentEnvelope {
    #[error(status = NOT_FOUND)]
    Missing,
    #[error(transparent)]
    Framework(#[from] Rejection),
}

#[r2e_core::test]
async fn transparent_over_rejection_is_the_rejection_variant() {
    let err: TransparentEnvelope = Rejection::unauthenticated().into();
    assert!(matches!(err, TransparentEnvelope::Framework(_)));
    assert_eq!(err.to_string(), "Unauthorized");
    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthorized");
    assert_eq!(
        TransparentEnvelope::extra_statuses(),
        vec![(StatusCode::NOT_FOUND, "Missing")]
    );
}

// An enum transparent over `HttpError` inherits its projection.
#[derive(Debug, ApiError)]
pub enum OverHttpError {
    #[error(status = BAD_GATEWAY)]
    Upstream,
    #[error(transparent)]
    Http(#[from] HttpError),
}

#[r2e_core::test]
async fn transparent_over_http_error_inherits_projection() {
    let err = OverHttpError::from(Rejection::not_found("gone"));
    assert!(matches!(err, OverHttpError::Http(HttpError::NotFound(_))));
    let (status, body) = error_parts(err).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "gone");

    // `From<HttpError>` from `#[from]` is still there.
    let err = OverHttpError::from(HttpError::Forbidden("no".into()));
    assert!(matches!(err, OverHttpError::Http(_)));

    assert!(OverHttpError::opaque_passthrough());
    assert_eq!(OverHttpError::body_schema(), HttpError::body_schema());
    assert_eq!(
        OverHttpError::extra_statuses(),
        vec![(StatusCode::BAD_GATEWAY, "Upstream")]
    );
}

// Generic enums get the impls too.
#[derive(Debug, ApiError)]
pub enum GenericEnvelope<T: std::fmt::Debug + Send + Sync + 'static> {
    #[error(status = CONFLICT)]
    Conflict(T),
    #[error(rejection)]
    Rejected(Rejection),
}

#[test]
fn generic_envelope_projects() {
    let err: GenericEnvelope<u32> = Rejection::internal("x").into();
    assert!(matches!(err, GenericEnvelope::Rejected(_)));
    assert_eq!(
        <GenericEnvelope<u32>>::extra_statuses(),
        vec![(StatusCode::CONFLICT, "Conflict")]
    );
}
