//! Projection of a framework [`Rejection`] onto a [`tonic::Status`].
//!
//! The gRPC wire has no HTTP status; a rejection is mapped **by kind** to the
//! closest gRPC code (the table on [`rejection_to_status`]). This is the
//! gRPC counterpart of `From<Rejection> for HttpError` / `McpError` — a free
//! function rather than a `From` impl because `tonic::Status` is foreign to
//! this crate (orphan rule).

use r2e_core::http::StatusCode;
use r2e_core::{Rejection, RejectionKind};
use tonic::metadata::MetadataMap;
use tonic::{Code, Status};

/// Map a [`Rejection`] onto a [`Status`] **by kind**:
///
/// | kind | `tonic::Code` |
/// |---|---|
/// | `Unauthenticated` | `Unauthenticated` |
/// | `Forbidden` | `PermissionDenied` |
/// | `NotFound` | `NotFound` |
/// | `Conflict` | `Aborted` |
/// | `RateLimited`, `PayloadTooLarge` | `ResourceExhausted` |
/// | `Unavailable` | `Unavailable` |
/// | `Timeout` | `DeadlineExceeded` |
/// | `Internal` | `Internal` |
/// | every request-shape kind (`MissingContentType`, `UnsupportedMediaType`, `BodyRead`, `MalformedBody`, `InvalidBody`, `InvalidPath`, `InvalidQuery`, `InvalidForm`, `InvalidHeader`, `BadRequest`, `Validation`) | `InvalidArgument` |
/// | `Opaque` (and future kinds) | by [`Rejection::status`] via [`code_from_status`] |
///
/// The rejection message becomes the status message (an empty one falls back
/// to `request rejected with status N`); [`Rejection::headers`] travel as
/// trailing metadata, so a `WWW-Authenticate` or `Retry-After` set by a guard
/// reaches the client.
#[must_use]
pub fn rejection_to_status(rejection: Rejection) -> Status {
    let code = match rejection.kind {
        RejectionKind::Unauthenticated => Code::Unauthenticated,
        RejectionKind::Forbidden => Code::PermissionDenied,
        RejectionKind::NotFound => Code::NotFound,
        RejectionKind::Conflict => Code::Aborted,
        RejectionKind::RateLimited | RejectionKind::PayloadTooLarge => Code::ResourceExhausted,
        RejectionKind::Unavailable => Code::Unavailable,
        RejectionKind::Timeout => Code::DeadlineExceeded,
        RejectionKind::Internal => Code::Internal,
        RejectionKind::MissingContentType
        | RejectionKind::UnsupportedMediaType
        | RejectionKind::BodyRead
        | RejectionKind::MalformedBody
        | RejectionKind::InvalidBody
        | RejectionKind::InvalidPath
        | RejectionKind::InvalidQuery
        | RejectionKind::InvalidForm
        | RejectionKind::InvalidHeader
        | RejectionKind::BadRequest
        | RejectionKind::Validation => Code::InvalidArgument,
        // `Opaque` carries a foreign response whose status is all we know;
        // `RejectionKind` is #[non_exhaustive], so future kinds degrade the
        // same way instead of breaking this crate.
        _ => code_from_status(rejection.status),
    };
    let message = if rejection.message.is_empty() {
        format!("request rejected with status {}", rejection.status.as_u16())
    } else {
        rejection.message.into_owned()
    };
    if rejection.headers.is_empty() {
        Status::new(code, message)
    } else {
        Status::with_metadata(code, message, MetadataMap::from_headers(rejection.headers))
    }
}

/// The status-only fallback of [`rejection_to_status`]: the gRPC code an HTTP
/// status maps to when no finer kind is known (401 → `Unauthenticated`,
/// 403 → `PermissionDenied`, 404 → `NotFound`, 409 → `Aborted`,
/// 429 → `ResourceExhausted`, other 4xx → `InvalidArgument`,
/// 503 → `Unavailable`, 504 → `DeadlineExceeded`, anything else → `Internal`).
#[must_use]
pub fn code_from_status(status: StatusCode) -> Code {
    match status {
        StatusCode::UNAUTHORIZED => Code::Unauthenticated,
        StatusCode::FORBIDDEN => Code::PermissionDenied,
        StatusCode::NOT_FOUND => Code::NotFound,
        StatusCode::CONFLICT => Code::Aborted,
        StatusCode::TOO_MANY_REQUESTS => Code::ResourceExhausted,
        StatusCode::SERVICE_UNAVAILABLE => Code::Unavailable,
        StatusCode::GATEWAY_TIMEOUT => Code::DeadlineExceeded,
        s if s.is_client_error() => Code::InvalidArgument,
        _ => Code::Internal,
    }
}
