//! [`ErrorSchema`]: the static side of an error envelope.

use crate::http::StatusCode;
use serde_json::Value;

use super::rejection::RejectionKind;

/// Static documentation of an error envelope.
///
/// Mandatory for every type used as an envelope — the `E` of a route's
/// `Result<T, E>` return type, or `AppBuilder::error_projection::<E>()` —
/// next to `From<Rejection>` and `IntoHttpResponse`. The runtime
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
    fn body_schema() -> Option<(String, Value)>;

    /// Per-kind body when one envelope has several shapes (`HttpError`
    /// answers `ValidationErrorResponse` to `Validation`). `None` falls back
    /// to [`body_schema`](Self::body_schema).
    fn body_schema_for(_kind: RejectionKind) -> Option<(String, Value)> {
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

/// A runtime capture of an [`ErrorSchema`] implementation.
///
/// Built by [`ErrorSchemaInfo::of`] and carried by
/// [`RouteInfo`](crate::di::meta::RouteInfo) (the route's own envelope) and
/// [`ErrorProjector`](super::ErrorProjector) (the application envelope), so
/// the OpenAPI builder documents error responses from the **same** table the
/// runtime projects through — without naming `E` at the type level.
#[derive(Clone, Copy)]
pub struct ErrorSchemaInfo {
    type_name: &'static str,
    status_of: fn(RejectionKind) -> StatusCode,
    body_schema: fn() -> Option<(String, Value)>,
    body_schema_for: fn(RejectionKind) -> Option<(String, Value)>,
    extra_statuses: fn() -> Vec<(StatusCode, &'static str)>,
    opaque_passthrough: bool,
}

impl ErrorSchemaInfo {
    /// Capture the envelope `E`.
    #[must_use]
    pub fn of<E: ErrorSchema + ?Sized>() -> Self {
        Self {
            type_name: std::any::type_name::<E>(),
            status_of: E::status_of,
            body_schema: E::body_schema,
            body_schema_for: E::body_schema_for,
            extra_statuses: E::extra_statuses,
            opaque_passthrough: E::opaque_passthrough(),
        }
    }

    /// The envelope's Rust type name (diagnostics only).
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// See [`ErrorSchema::status_of`].
    #[must_use]
    pub fn status_of(&self, kind: RejectionKind) -> StatusCode {
        (self.status_of)(kind)
    }

    /// See [`ErrorSchema::body_schema`].
    #[must_use]
    pub fn body_schema(&self) -> Option<(String, Value)> {
        (self.body_schema)()
    }

    /// The body documented for `kind`: [`ErrorSchema::body_schema_for`] when
    /// it answers, else [`ErrorSchema::body_schema`].
    #[must_use]
    pub fn body_schema_for(&self, kind: RejectionKind) -> Option<(String, Value)> {
        (self.body_schema_for)(kind).or_else(|| (self.body_schema)())
    }

    /// See [`ErrorSchema::extra_statuses`].
    #[must_use]
    pub fn extra_statuses(&self) -> Vec<(StatusCode, &'static str)> {
        (self.extra_statuses)()
    }

    /// See [`ErrorSchema::opaque_passthrough`].
    #[must_use]
    pub fn opaque_passthrough(&self) -> bool {
        self.opaque_passthrough
    }
}

impl std::fmt::Debug for ErrorSchemaInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ErrorSchemaInfo({})", self.type_name)
    }
}
