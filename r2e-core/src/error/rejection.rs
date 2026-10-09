//! The typed request-failure hub: [`Rejection`] and [`RejectionKind`].
//!
//! Every way a request can fail before (or around) the handler — extractor
//! rejections, identity extraction, guards, validation, managed resources —
//! converts into a [`Rejection`] through a plain `From` impl, and a route's
//! error envelope `E: From<Rejection> + IntoHttpResponse + ErrorSchema` turns it
//! into the wire response **once**, at a single projection point
//! ([`Rejection::project`]). See `plans/error-projection.md`.
//!
//! Nothing here renders a body: a `Rejection` is data. [`HttpError`] is the
//! default envelope and reproduces the 0.4 bodies exactly.

use std::borrow::Cow;
use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use crate::http::extract::rejection::{
    BytesRejection, ExtensionRejection, FormRejection, MatchedPathRejection, NestedPathRejection,
    PathRejection, QueryRejection, RawFormRejection, RawPathParamsRejection, StringRejection,
};
use crate::http::header::{HeaderMap, HeaderName, HeaderValue};
use crate::http::response::{IntoHttpResponse, Response};
use crate::http::{JsonRejection, StatusCode};

use super::schema::ErrorSchema;
use super::HttpError;

/// What went wrong, independently of how it is rendered.
///
/// Each kind has one default status ([`RejectionKind::default_status`]); an
/// envelope remaps a kind through [`ErrorSchema::status_of`].
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize)]
pub enum RejectionKind {
    // ── request shape ──
    /// No `Content-Type` on a request that needs one (415).
    MissingContentType,
    /// A `Content-Type` the extractor cannot handle (415).
    UnsupportedMediaType,
    /// The body exceeded a size limit (413).
    PayloadTooLarge,
    /// The body could not be read from the transport (400).
    BodyRead,
    /// The body is not well-formed for its format — JSON syntax, early EOF (400).
    MalformedBody,
    /// The body parsed but does not fit the target type (422).
    InvalidBody,
    /// A path parameter is missing or cannot be parsed (400).
    InvalidPath,
    /// A query parameter is missing or cannot be parsed (400).
    InvalidQuery,
    /// A form field is missing or cannot be parsed (400).
    InvalidForm,
    /// A header is missing or cannot be parsed (400).
    InvalidHeader,
    /// A bad request that fits no finer kind (400).
    BadRequest,
    /// The payload was accepted but failed `garde` validation — field errors
    /// in [`Rejection::details`] (400).
    Validation,
    // ── auth ──
    /// No usable identity on the request (401).
    Unauthenticated,
    /// Authenticated but not allowed (403).
    Forbidden,
    // ── resource ──
    /// The target does not exist (404).
    NotFound,
    /// The request conflicts with the current state (409).
    Conflict,
    /// A rate limit was hit — `Retry-After` in [`Rejection::headers`] when
    /// known (429).
    RateLimited,
    // ── server ──
    /// Something failed on our side (500).
    Internal,
    /// A dependency is down or not provisioned (503).
    Unavailable,
    /// A dependency took too long (504).
    Timeout,
    /// A response the framework could not type — a foreign extractor's
    /// rejection or a guard that built its own `Response`. The original
    /// response rides along privately and is passed through by envelopes whose
    /// [`ErrorSchema::opaque_passthrough`] is `true` (`HttpError` is).
    Opaque,
}

impl RejectionKind {
    /// The one kind → status table.
    ///
    /// `Opaque` has no meaningful default; it answers 500 and the carried
    /// response's status always wins (see [`Rejection::status`]).
    #[must_use]
    pub const fn default_status(self) -> StatusCode {
        match self {
            Self::MissingContentType | Self::UnsupportedMediaType => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::BodyRead
            | Self::MalformedBody
            | Self::InvalidPath
            | Self::InvalidQuery
            | Self::InvalidForm
            | Self::InvalidHeader
            | Self::BadRequest
            | Self::Validation => StatusCode::BAD_REQUEST,
            Self::InvalidBody => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal | Self::Opaque => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    /// The closest kind for a bare status, for faults that only know their
    /// status (`HttpError::Custom`, `GuardError`, a foreign rejection).
    ///
    /// Unlisted 4xx map to [`BadRequest`](Self::BadRequest) and unlisted 5xx
    /// to [`Internal`](Self::Internal); the status itself is preserved on the
    /// [`Rejection`], so a 418 stays a 418.
    #[must_use]
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Self::UnsupportedMediaType,
            StatusCode::PAYLOAD_TOO_LARGE => Self::PayloadTooLarge,
            StatusCode::UNPROCESSABLE_ENTITY => Self::InvalidBody,
            StatusCode::UNAUTHORIZED => Self::Unauthenticated,
            StatusCode::FORBIDDEN => Self::Forbidden,
            StatusCode::NOT_FOUND => Self::NotFound,
            StatusCode::CONFLICT => Self::Conflict,
            StatusCode::TOO_MANY_REQUESTS => Self::RateLimited,
            StatusCode::SERVICE_UNAVAILABLE => Self::Unavailable,
            StatusCode::GATEWAY_TIMEOUT => Self::Timeout,
            s if s.is_client_error() => Self::BadRequest,
            _ => Self::Internal,
        }
    }

    /// Every kind, in declaration order — for exhaustive tests and spec
    /// generation.
    pub const ALL: &'static [RejectionKind] = &[
        Self::MissingContentType,
        Self::UnsupportedMediaType,
        Self::PayloadTooLarge,
        Self::BodyRead,
        Self::MalformedBody,
        Self::InvalidBody,
        Self::InvalidPath,
        Self::InvalidQuery,
        Self::InvalidForm,
        Self::InvalidHeader,
        Self::BadRequest,
        Self::Validation,
        Self::Unauthenticated,
        Self::Forbidden,
        Self::NotFound,
        Self::Conflict,
        Self::RateLimited,
        Self::Internal,
        Self::Unavailable,
        Self::Timeout,
        Self::Opaque,
    ];
}

/// A typed request failure, before it is rendered.
///
/// Built by the `From` impls below (one per fault type) or by the constructors;
/// consumed by an envelope's `From<Rejection>` impl, which **must** read
/// [`status`](Self::status) rather than the kind table — the framework has
/// already applied the envelope's [`ErrorSchema::status_of`] remap to it.
///
/// Not `Clone`: an [`Opaque`](RejectionKind::Opaque) rejection owns a
/// `Response`.
#[non_exhaustive]
pub struct Rejection {
    /// What failed.
    pub kind: RejectionKind,
    /// Status to answer with. Starts at the status the fault carried (usually
    /// `kind.default_status()`); [`Rejection::project`] overwrites it with the
    /// envelope's remap when the envelope remaps this kind.
    pub status: StatusCode,
    /// Client-facing message. Must not leak internals.
    pub message: Cow<'static, str>,
    /// Structured payload — validation field errors, …
    pub details: Option<serde_json::Value>,
    /// Headers the response must carry (`WWW-Authenticate`, `Retry-After`, …).
    /// Applied to the projected response by [`Rejection::project`].
    pub headers: HeaderMap,
    /// The underlying cause, for logging. Never rendered.
    pub source: Option<Arc<dyn StdError + Send + Sync>>,
    /// The pre-rendered response of an `Opaque` rejection.
    opaque: Option<Response>,
}

impl Rejection {
    /// A rejection of `kind` at its default status.
    pub fn new(kind: RejectionKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            status: kind.default_status(),
            message: message.into(),
            details: None,
            headers: HeaderMap::new(),
            source: None,
            opaque: None,
        }
    }

    /// A rejection whose status is not the kind's default — a fault that
    /// carries its own status (`HttpError::Custom`, configurable tenant
    /// statuses, a body-read failure that was a 413).
    pub fn with_status(
        kind: RejectionKind,
        status: StatusCode,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        let mut r = Self::new(kind, message);
        r.status = status;
        r
    }

    /// A rejection from a bare status: kind chosen by
    /// [`RejectionKind::from_status`], status preserved.
    pub fn from_status(status: StatusCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::with_status(RejectionKind::from_status(status), status, message)
    }

    /// 401 with the fixed message `Unauthorized`. Allocation-free.
    #[must_use]
    pub fn unauthenticated() -> Self {
        Self::new(RejectionKind::Unauthenticated, "Unauthorized")
    }

    /// 403.
    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(RejectionKind::Forbidden, message)
    }

    /// 404.
    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(RejectionKind::NotFound, message)
    }

    /// 400 with no finer kind.
    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(RejectionKind::BadRequest, message)
    }

    /// 500. The message is what the client sees; keep internals in
    /// [`source`](Self::source).
    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(RejectionKind::Internal, message)
    }

    /// Wrap an already-rendered response. Its status is kept; the message is
    /// the status' canonical reason (`Not Found`, …) so a non-passthrough
    /// envelope still has something to say.
    #[must_use]
    pub fn opaque(response: Response) -> Self {
        let status = response.status();
        let message: Cow<'static, str> = match status.canonical_reason() {
            Some(reason) => Cow::Borrowed(reason),
            None => Cow::Owned(format!("request rejected with status {status}")),
        };
        let mut r = Self::with_status(RejectionKind::Opaque, status, message);
        r.opaque = Some(response);
        r
    }

    /// Attach structured details.
    #[must_use]
    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Add a response header.
    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Attach the underlying cause.
    #[must_use]
    pub fn source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// Attach an already-shared cause.
    #[must_use]
    pub fn source_arc(mut self, source: Arc<dyn StdError + Send + Sync>) -> Self {
        self.source = Some(source);
        self
    }

    /// Take the pre-rendered response of an `Opaque` rejection, if any.
    ///
    /// Also the only way to see whether the rejection was built from a
    /// response, since `opaque` is private. After this call the rejection is a
    /// plain `Opaque` with status and message.
    #[must_use]
    pub fn take_opaque(&mut self) -> Option<Response> {
        self.opaque.take()
    }

    /// Whether this rejection carries a pre-rendered response.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.opaque.is_some()
    }

    /// **The projection point.** Turn this rejection into the wire response
    /// through the envelope `E`:
    ///
    /// 1. if `E::status_of(kind)` differs from the kind's default, it
    ///    overwrites [`status`](Self::status) (a status the fault itself
    ///    carried — a 413 body read, a configured tenant status — survives an
    ///    envelope that does not remap the kind);
    /// 2. an `Opaque` rejection is returned as-is when
    ///    `E::opaque_passthrough()`;
    /// 3. otherwise `E::from(self).into_http_response()`, with
    ///    [`headers`](Self::headers) added to the result (an envelope's own
    ///    header of the same name wins).
    ///
    /// Generated handlers call this; so does `From<Rejection> for Response`
    /// (with `E = HttpError`).
    pub fn project<E>(mut self) -> Response
    where
        E: From<Rejection> + IntoHttpResponse + ErrorSchema,
    {
        let remapped = E::status_of(self.kind);
        if remapped != self.kind.default_status() {
            self.status = remapped;
        }
        if E::opaque_passthrough() {
            if let Some(mut response) = self.opaque.take() {
                if self.status != response.status() {
                    *response.status_mut() = self.status;
                }
                return response;
            }
        }
        let headers = std::mem::take(&mut self.headers);
        let mut response = E::from(self).into_http_response();
        if !headers.is_empty() {
            let target = response.headers_mut();
            for (name, value) in headers {
                // `HeaderMap::into_iter` yields `None` for the 2nd+ value of a
                // repeated name; we only set each name once anyway.
                if let Some(name) = name {
                    target.entry(name).or_insert(value);
                }
            }
        }
        response
    }
}

impl fmt::Debug for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rejection")
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("message", &self.message)
            .field("details", &self.details)
            .field("headers", &self.headers)
            .field("source", &self.source.as_ref().map(|s| s.to_string()))
            .field("opaque", &self.opaque.is_some())
            .finish()
    }
}

/// The client-facing message only — an envelope's `Display` delegates here
/// (`#[error(rejection)]`), so it must not leak status/kind noise. `Debug`
/// has the full picture.
impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for Rejection {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|s| s as &(dyn StdError + 'static))
    }
}

/// Default projection: through [`HttpError`] (the 0.4 bodies).
///
/// Legal despite `Response` being foreign because `Rejection` is local. Used
/// by hand-written axum handlers merged via `merge_router`; generated routes
/// project through [`Rejection::project`] with the route's envelope instead.
impl From<Rejection> for Response {
    fn from(rejection: Rejection) -> Self {
        rejection.project::<HttpError>()
    }
}

impl IntoHttpResponse for Rejection {
    fn into_http_response(self) -> Response {
        self.into()
    }
}

crate::http::impl_into_response!(Rejection);

// ── faults → hub ──────────────────────────────────────────────────────────

impl From<Infallible> for Rejection {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

impl From<Response> for Rejection {
    fn from(response: Response) -> Self {
        Self::opaque(response)
    }
}

impl From<JsonRejection> for Rejection {
    fn from(r: JsonRejection) -> Self {
        let (kind, status) = match &r {
            JsonRejection::MissingContentType => (
                RejectionKind::MissingContentType,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            JsonRejection::BodyRead { status, .. } => {
                if *status == StatusCode::PAYLOAD_TOO_LARGE {
                    (RejectionKind::PayloadTooLarge, *status)
                } else {
                    (RejectionKind::BodyRead, *status)
                }
            }
            JsonRejection::Syntax(_) | JsonRejection::Eof(_) => {
                (RejectionKind::MalformedBody, StatusCode::BAD_REQUEST)
            }
            JsonRejection::Data(_) => (
                RejectionKind::InvalidBody,
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        };
        let message = r.body_text();
        let mut rejection = Self::with_status(kind, status, message);
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<PathRejection> for Rejection {
    fn from(r: PathRejection) -> Self {
        // `MissingPathParams` is a 500 (route/extractor mismatch): keep it.
        let mut rejection = Self::with_status(RejectionKind::InvalidPath, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<QueryRejection> for Rejection {
    fn from(r: QueryRejection) -> Self {
        let mut rejection =
            Self::with_status(RejectionKind::InvalidQuery, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<FormRejection> for Rejection {
    fn from(r: FormRejection) -> Self {
        let status = r.status();
        let kind = match status {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => RejectionKind::UnsupportedMediaType,
            StatusCode::PAYLOAD_TOO_LARGE => RejectionKind::PayloadTooLarge,
            _ => RejectionKind::InvalidForm,
        };
        let mut rejection = Self::with_status(kind, status, r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

#[cfg(feature = "multipart")]
impl From<crate::http::multipart::MultipartRejection> for Rejection {
    fn from(r: crate::http::multipart::MultipartRejection) -> Self {
        let status = r.status();
        let kind = match status {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => RejectionKind::UnsupportedMediaType,
            StatusCode::PAYLOAD_TOO_LARGE => RejectionKind::PayloadTooLarge,
            _ => RejectionKind::MalformedBody,
        };
        let mut rejection = Self::with_status(kind, status, r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

#[cfg(feature = "multipart")]
impl From<crate::web::multipart::MultipartError> for Rejection {
    fn from(e: crate::web::multipart::MultipartError) -> Self {
        use crate::web::multipart::MultipartError as M;
        let kind = match &e {
            M::FieldTooLarge { .. } | M::PayloadTooLarge { .. } => RejectionKind::PayloadTooLarge,
            // A missing/unparsable field answered 400 in 0.4; `MalformedBody`
            // keeps that status (and keeps the spec's inferred status right).
            M::MissingField(_) | M::ParseError { .. } | M::AxumError(_) | M::ReadError(_) => {
                RejectionKind::MalformedBody
            }
        };
        let mut rejection = Self::new(kind, e.to_string());
        rejection.source = Some(Arc::new(e));
        rejection
    }
}

impl From<crate::web::params::ParamError> for Rejection {
    fn from(e: crate::web::params::ParamError) -> Self {
        use crate::web::params::ParamLocation;
        let kind = match e.location {
            ParamLocation::Path => RejectionKind::InvalidPath,
            ParamLocation::Query => RejectionKind::InvalidQuery,
            ParamLocation::Header => RejectionKind::InvalidHeader,
        };
        Self::new(kind, e.message)
    }
}

impl From<crate::web::validation::ValidationErrorResponse> for Rejection {
    fn from(v: crate::web::validation::ValidationErrorResponse) -> Self {
        let details = serde_json::to_value(&v.errors).unwrap_or(serde_json::Value::Null);
        Self::new(RejectionKind::Validation, "Validation failed").details(details)
    }
}

impl From<&garde::Report> for Rejection {
    fn from(report: &garde::Report) -> Self {
        crate::web::validation::ValidationErrorResponse::from_report(report).into()
    }
}

impl From<garde::Report> for Rejection {
    fn from(report: garde::Report) -> Self {
        Self::from(&report)
    }
}

/// Body read failures of the raw-body extractors (`Bytes`, `String`): a 413
/// over the body limit, otherwise a 400 (`BodyRead`); invalid UTF-8 in a
/// `String` body is `MalformedBody`.
impl From<BytesRejection> for Rejection {
    fn from(r: BytesRejection) -> Self {
        let status = r.status();
        let kind = if status == StatusCode::PAYLOAD_TOO_LARGE {
            RejectionKind::PayloadTooLarge
        } else {
            RejectionKind::BodyRead
        };
        let mut rejection = Self::with_status(kind, status, r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<StringRejection> for Rejection {
    fn from(r: StringRejection) -> Self {
        let status = r.status();
        let kind = match &r {
            StringRejection::InvalidUtf8(_) => RejectionKind::MalformedBody,
            _ if status == StatusCode::PAYLOAD_TOO_LARGE => RejectionKind::PayloadTooLarge,
            _ => RejectionKind::BodyRead,
        };
        let mut rejection = Self::with_status(kind, status, r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<RawFormRejection> for Rejection {
    fn from(r: RawFormRejection) -> Self {
        let kind = match &r {
            RawFormRejection::InvalidFormContentType(_) => RejectionKind::UnsupportedMediaType,
            _ => RejectionKind::InvalidForm,
        };
        let mut rejection = Self::with_status(kind, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

/// A missing `Extension<T>` is a wiring error (500), kept as `Internal`.
impl From<ExtensionRejection> for Rejection {
    fn from(r: ExtensionRejection) -> Self {
        let mut rejection = Self::with_status(RejectionKind::Internal, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

/// `MatchedPath` / `NestedPath` outside a matched route: server-side misuse.
impl From<MatchedPathRejection> for Rejection {
    fn from(r: MatchedPathRejection) -> Self {
        let mut rejection = Self::with_status(RejectionKind::Internal, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<NestedPathRejection> for Rejection {
    fn from(r: NestedPathRejection) -> Self {
        let mut rejection = Self::with_status(RejectionKind::Internal, r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

/// Raw path params: invalid UTF-8 is the client's (400, `InvalidPath`); a
/// missing param set is a route/extractor mismatch (500).
impl From<RawPathParamsRejection> for Rejection {
    fn from(r: RawPathParamsRejection) -> Self {
        let status = r.status();
        let kind = if status.is_server_error() {
            RejectionKind::Internal
        } else {
            RejectionKind::InvalidPath
        };
        let mut rejection = Self::with_status(kind, status, r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

/// The common hand-written axum rejection shape, `(StatusCode, message)`.
impl From<(StatusCode, &'static str)> for Rejection {
    fn from((status, message): (StatusCode, &'static str)) -> Self {
        Self::from_status(status, message)
    }
}

impl From<(StatusCode, String)> for Rejection {
    fn from((status, message): (StatusCode, String)) -> Self {
        Self::from_status(status, message)
    }
}

/// A failed WebSocket handshake (`WebSocketUpgrade` extraction): kind chosen
/// from the status axum assigns (400/405/426), message = axum's body text.
#[cfg(feature = "ws")]
impl From<crate::http::ws::WebSocketUpgradeRejection> for Rejection {
    fn from(r: crate::http::ws::WebSocketUpgradeRejection) -> Self {
        let mut rejection = Self::from_status(r.status(), r.body_text());
        rejection.source = Some(Arc::new(r));
        rejection
    }
}

impl From<crate::decorators::guards::GuardError> for Rejection {
    fn from(e: crate::decorators::guards::GuardError) -> Self {
        Self::from_status(e.status, e.message)
    }
}

impl From<HttpError> for Rejection {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::NotFound(m) => Self::new(RejectionKind::NotFound, m),
            HttpError::Unauthorized(m) => Self::new(RejectionKind::Unauthenticated, m),
            HttpError::Forbidden(m) => Self::new(RejectionKind::Forbidden, m),
            HttpError::BadRequest(m) => Self::new(RejectionKind::BadRequest, m),
            HttpError::Internal(m) => Self::new(RejectionKind::Internal, m),
            HttpError::Validation(v) => v.into(),
            HttpError::Custom { status, body } => {
                let message: Cow<'static, str> = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| Cow::Owned(s.to_owned()))
                    .unwrap_or_else(|| {
                        Cow::Borrowed(status.canonical_reason().unwrap_or("Error"))
                    });
                Self::from_status(status, message).details(body)
            }
            HttpError::WithSource {
                status,
                message,
                source,
            } => Self::from_status(status, message).source_arc(source),
        }
    }
}
