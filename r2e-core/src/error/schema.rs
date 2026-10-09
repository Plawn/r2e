//! [`ErrorSchema`]: the static side of an error envelope.

use crate::http::StatusCode;

use super::rejection::RejectionKind;

/// Static documentation of an error envelope.
///
/// Mandatory for every type used as a projector (`#[error(E)]`,
/// `#[routes(error = E)]`, `AppBuilder::error_projection::<E>()`), next to
/// `From<Rejection>` and `IntoHttpResponse`. The runtime
/// ([`Rejection::project`](super::Rejection::project)) and the OpenAPI builder
/// read the **same** [`status_of`](Self::status_of), so the spec cannot say
/// 422 where the server answers 400.
///
/// `HttpError` implements it (the default envelope) and `#[derive(ApiError)]`
/// emits it for an enum with an `#[error(rejection)]` variant.
pub trait ErrorSchema {
    /// Status this envelope emits for `kind`.
    ///
    /// Default: [`RejectionKind::default_status`]. Override to remap — an
    /// OpenAI-shaped API answers 400 to every malformed request, so it maps
    /// `InvalidBody` (422 by default) to 400. A kind left at its default
    /// keeps the status the fault carried (a 413 body read stays a 413).
    #[must_use]
    fn status_of(kind: RejectionKind) -> StatusCode {
        kind.default_status()
    }

    /// `(component name, JSON Schema)` of the error body. `None` when the
    /// body is undocumented.
    fn body_schema() -> Option<(String, serde_json::Value)>;

    /// Per-kind body when one envelope has several shapes (`HttpError`
    /// answers `ValidationErrorResponse` to `Validation`). `None` falls back
    /// to [`body_schema`](Self::body_schema).
    fn body_schema_for(_kind: RejectionKind) -> Option<(String, serde_json::Value)> {
        None
    }

    /// Statuses the envelope can emit that no inferred kind covers — a 404
    /// from a handler's own error, a 502 from a proxy — as
    /// `(status, description)`.
    fn extra_statuses() -> Vec<(StatusCode, &'static str)> {
        Vec::new()
    }

    /// Whether an [`Opaque`](RejectionKind::Opaque) rejection — a response the
    /// framework could not type — is returned untouched instead of being
    /// re-wrapped in this envelope.
    ///
    /// `false` by default: an envelope that promises one body shape wraps
    /// everything (the opaque body is dropped; status and reason are kept).
    /// `HttpError` returns `true`, which is the 0.4 behaviour.
    #[must_use]
    fn opaque_passthrough() -> bool {
        false
    }
}
