//! The JSON codec façade, and R2E's own `Json<T>`.
//!
//! This module is the *one* place in the workspace that performs JSON
//! (de)serialization of typed values. Everything else — the `Json<T>`
//! extractor/response, `HttpError` bodies, WebSocket/SSE payloads, event-bus
//! envelopes, `#[derive(Cacheable)]` — calls [`to_vec`] / [`from_slice`] & co.
//! here, so swapping the codec is a change to this file and a Cargo feature,
//! not a sweep over sixty call sites. See `plans/json-codec-containment.md`.
//!
//! # What is *not* abstracted
//!
//! `serde_json::Value` and `json!` are the workspace's dynamic-tree type and
//! stay `serde_json` (plan §1.3): a SIMD codec wins on typed structs, not on
//! a boxed tree, and each brings its own incompatible `Value`. The boundary
//! is about **who does the work**, not about the name of the tree type.
//! [`JsonError`] implements `From<serde_json::Error>` so the `Value` paths
//! (`serde_json::to_value`, `json!`) keep composing with the façade.
//!
//! # Backends
//!
//! - default: `serde_json`.
//! - feature `json-sonic`: `sonic-rs` (SIMD). Additive — when enabled it takes
//!   precedence. Both accept `&[u8]` input, so the extractor never copies the
//!   body; that is why `simd-json` (which wants `&mut [u8]`) is not offered.
//!
//! No `to_writer`: the backends disagree on the writer bound, and nothing in
//! the workspace streams a serializer into a writer — `to_vec` into a `Bytes`
//! body is the shape every response takes.

use std::fmt;
use std::ops::{Deref, DerefMut};

use axum::extract::FromRequest;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::response::{IntoHttpResponse, IntoResponse, Response};
use crate::{HeaderValue, Request, StatusCode, CONTENT_TYPE};

// ── backend ──────────────────────────────────────────────────────────────────

#[cfg(feature = "json-sonic")]
mod backend {
    pub use sonic_rs::{from_slice, from_str, to_string, to_vec, Error};
    pub const NAME: &str = "sonic-rs";

    pub fn kind(e: &Error) -> super::JsonErrorKind {
        use super::JsonErrorKind::*;
        if e.is_unmatched_type() {
            Data
        } else if e.is_eof() {
            Eof
        } else if e.is_io() {
            Io
        } else {
            Syntax
        }
    }
}

#[cfg(not(feature = "json-sonic"))]
mod backend {
    pub use serde_json::{from_slice, from_str, to_string, to_vec, Error};
    pub const NAME: &str = "serde_json";

    pub fn kind(e: &Error) -> super::JsonErrorKind {
        super::serde_json_kind(e)
    }
}

fn serde_json_kind(e: &serde_json::Error) -> JsonErrorKind {
    if e.is_data() {
        JsonErrorKind::Data
    } else if e.is_eof() {
        JsonErrorKind::Eof
    } else if e.is_io() {
        JsonErrorKind::Io
    } else {
        JsonErrorKind::Syntax
    }
}

/// Name of the codec backend compiled in (`"serde_json"` or `"sonic-rs"`).
pub const BACKEND: &str = backend::NAME;

// ── functions ────────────────────────────────────────────────────────────────

/// Serialize `value` to a JSON byte vector.
#[inline]
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, JsonError> {
    backend::to_vec(value).map_err(JsonError::from_backend)
}

/// Serialize `value` to a JSON string.
#[inline]
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String, JsonError> {
    backend::to_string(value).map_err(JsonError::from_backend)
}

/// Deserialize a `T` from JSON bytes.
#[inline]
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, JsonError> {
    backend::from_slice(bytes).map_err(JsonError::from_backend)
}

/// Deserialize a `T` from a JSON string.
#[inline]
pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T, JsonError> {
    backend::from_str(s).map_err(JsonError::from_backend)
}

// ── error ────────────────────────────────────────────────────────────────────

/// Why a JSON (de)serialization failed.
///
/// The distinction that matters to HTTP: [`Data`](JsonErrorKind::Data) is
/// well-formed JSON of the wrong shape (a `422 Unprocessable Entity` on the
/// extractor), everything else is a malformed document (`400`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonErrorKind {
    /// Malformed JSON.
    Syntax,
    /// Well-formed JSON that does not match the target type.
    Data,
    /// Input ended before the document did.
    Eof,
    /// An I/O error from the underlying reader/writer.
    Io,
}

/// A JSON (de)serialization error, backend-neutral.
///
/// Wraps the compiled-in backend's error; the message and the
/// [`kind`](Self::kind) classification are stable across backends.
pub struct JsonError {
    kind: JsonErrorKind,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl JsonError {
    fn from_backend(e: backend::Error) -> Self {
        Self {
            kind: backend::kind(&e),
            source: Box::new(e),
        }
    }

    /// Classification of the failure.
    pub fn kind(&self) -> JsonErrorKind {
        self.kind
    }

    /// `true` for well-formed JSON of the wrong shape.
    pub fn is_data(&self) -> bool {
        self.kind == JsonErrorKind::Data
    }

    /// `true` for malformed JSON.
    pub fn is_syntax(&self) -> bool {
        self.kind == JsonErrorKind::Syntax
    }

    /// `true` when the input ended early.
    pub fn is_eof(&self) -> bool {
        self.kind == JsonErrorKind::Eof
    }

    /// `true` for I/O failures.
    pub fn is_io(&self) -> bool {
        self.kind == JsonErrorKind::Io
    }
}

impl fmt::Debug for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonError")
            .field("kind", &self.kind)
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.source, f)
    }
}

impl std::error::Error for JsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// The dynamic-tree paths (`serde_json::to_value`, `from_value`, `json!`)
/// stay on `serde_json`; this lets them flow into the façade's error type.
impl From<serde_json::Error> for JsonError {
    fn from(e: serde_json::Error) -> Self {
        Self {
            kind: serde_json_kind(&e),
            source: Box::new(e),
        }
    }
}

// ── Json<T> ──────────────────────────────────────────────────────────────────

/// JSON extractor and response.
///
/// As a handler parameter, `Json<T>` reads the request body (content type
/// must be `application/json` or a `+json` suffix) and deserializes it into
/// `T`; a failure is a [`JsonRejection`]. As a return value, it serializes
/// `T` with `Content-Type: application/json`.
///
/// R2E's own type since `plans/json-codec-containment.md` — it goes through
/// the [`json`](self) façade, so it follows the compiled-in codec. The
/// backend-side `FromRequest` impl below is a named bridge point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[must_use]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Json<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> From<T> for Json<T> {
    fn from(inner: T) -> Self {
        Json(inner)
    }
}

/// `application/json`, or any `<type>/<subtype>+json` (e.g.
/// `application/merge-patch+json`), parameters ignored.
fn is_json_content_type(headers: &crate::HeaderMap) -> bool {
    let Some(ct) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let essence = ct.split(';').next().unwrap_or("").trim();
    let Some((_ty, subtype)) = essence.split_once('/') else {
        return false;
    };
    essence.eq_ignore_ascii_case("application/json")
        || subtype.len() > 5 && subtype[subtype.len() - 5..].eq_ignore_ascii_case("+json")
}

/// Serialize `value` into a JSON response, or a 500 with the serializer's
/// message when it cannot be serialized (the same policy the backend had).
fn json_response<T: Serialize>(value: &T) -> Response {
    match to_vec(value) {
        Ok(bytes) => (
            [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            Bytes::from(bytes),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            e.to_string(),
        )
            .into_response(),
    }
}

impl<T: Serialize> IntoHttpResponse for Json<T> {
    fn into_http_response(self) -> Response {
        json_response(&self.0)
    }
}

// Generic type: `impl_into_response!` only covers non-generic ones, so the
// bridge to the backend's contract is written out here. Same shape.
impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        self.into_http_response()
    }
}

/// Bridge point: R2E's `Json<T>` speaking the backend's extraction contract
/// (`plans/runtime-http-dependency-containment.md` §5.3b table).
impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = JsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !is_json_content_type(req.headers()) {
            return Err(JsonRejection::MissingContentType);
        }
        let bytes = <Bytes as FromRequest<S>>::from_request(req, state)
            .await
            .map_err(JsonRejection::from_bytes_rejection)?;
        Self::from_bytes(&bytes)
    }
}

/// `Option<Json<T>>`: `None` when the request carries no JSON content type,
/// otherwise the same as `Json<T>` (a bad body is still a rejection).
impl<T, S> axum::extract::OptionalFromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = JsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        if !is_json_content_type(req.headers()) {
            return Ok(None);
        }
        let bytes = <Bytes as FromRequest<S>>::from_request(req, state)
            .await
            .map_err(JsonRejection::from_bytes_rejection)?;
        Self::from_bytes(&bytes).map(Some)
    }
}

impl<T: DeserializeOwned> Json<T> {
    /// Deserialize a body that has already been read. What the extractor
    /// does after the content-type check; usable directly from middleware or
    /// tests.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, JsonRejection> {
        from_slice(bytes).map(Json).map_err(|e| match e.kind() {
            JsonErrorKind::Data => JsonRejection::Data(e),
            JsonErrorKind::Eof => JsonRejection::Eof(e),
            JsonErrorKind::Syntax | JsonErrorKind::Io => JsonRejection::Syntax(e),
        })
    }
}

// ── rejection ────────────────────────────────────────────────────────────────

/// Why a `Json<T>` extraction was rejected. Each variant maps to the status
/// the backend used for the equivalent case, so behaviour is unchanged.
#[derive(Debug)]
pub enum JsonRejection {
    /// No `Content-Type: application/json` (415).
    MissingContentType,
    /// The body could not be read: too large (413) or a transport error (400).
    BodyRead {
        /// Status the body layer chose.
        status: StatusCode,
        /// Its message.
        message: String,
    },
    /// Malformed JSON (400).
    Syntax(JsonError),
    /// Well-formed JSON of the wrong shape (422).
    Data(JsonError),
    /// Body ended early (400).
    Eof(JsonError),
}

impl JsonRejection {
    fn from_bytes_rejection(r: axum::extract::rejection::BytesRejection) -> Self {
        JsonRejection::BodyRead {
            status: r.status(),
            message: r.body_text(),
        }
    }

    /// HTTP status this rejection answers with.
    pub fn status(&self) -> StatusCode {
        match self {
            JsonRejection::MissingContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            JsonRejection::BodyRead { status, .. } => *status,
            JsonRejection::Syntax(_) | JsonRejection::Eof(_) => StatusCode::BAD_REQUEST,
            JsonRejection::Data(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    /// Plain-text body of the response.
    pub fn body_text(&self) -> String {
        match self {
            JsonRejection::MissingContentType => {
                "Expected request with `Content-Type: application/json`".to_owned()
            }
            JsonRejection::BodyRead { message, .. } => message.clone(),
            JsonRejection::Syntax(e) => format!("Failed to parse the request body as JSON: {e}"),
            JsonRejection::Data(e) => {
                format!("Failed to deserialize the JSON body into the target type: {e}")
            }
            JsonRejection::Eof(e) => format!("Failed to parse the request body as JSON: {e}"),
        }
    }
}

impl fmt::Display for JsonRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.body_text())
    }
}

impl std::error::Error for JsonRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            JsonRejection::Syntax(e) | JsonRejection::Data(e) | JsonRejection::Eof(e) => Some(e),
            _ => None,
        }
    }
}

impl IntoHttpResponse for JsonRejection {
    fn into_http_response(self) -> Response {
        (
            self.status(),
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            self.body_text(),
        )
            .into_response()
    }
}

crate::impl_into_response!(JsonRejection);
