//! Framework [`RequestBodySchema`] / [`ResponseBodySchema`] impls.
//!
//! The `#[routes]` macro probes a handler's body extractor and its return type
//! for these traits; the impls below make R2E's own types self-describing so
//! that OpenAPI documents them without any name-based detection in the macro.
//! `Json<T>` is always documented (media type + rejection kinds); its JSON
//! schema needs schemars and therefore the `openapi` feature (enabled by
//! `r2e-openapi`). Without the feature the schema is `None`, so the metadata
//! shape never depends on features — only the schema's presence does.

use std::borrow::Cow;

use crate::di::meta::{RequestBodySchema, ResponseBodySchema, ResponseContent, OCTET_STREAM};
use crate::error::RejectionKind;
use crate::http::{Bytes, Form, HeaderMap, Html, Redirect, Sse, StatusCode};

use crate::http::Json;

// ── Request bodies ───────────────────────────────────────────────────────────

#[cfg(feature = "openapi")]
impl<T: schemars::JsonSchema> RequestBodySchema for Json<T> {
    fn content_type() -> &'static str {
        "application/json"
    }
    fn body_schema() -> Option<(String, serde_json::Value)> {
        Some(crate::di::meta::schema_of::<T>())
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        json_kinds()
    }
}

#[cfg(not(feature = "openapi"))]
impl<T> RequestBodySchema for Json<T> {
    fn content_type() -> &'static str {
        "application/json"
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        json_kinds()
    }
}

fn json_kinds() -> Vec<RejectionKind> {
    vec![
        RejectionKind::MissingContentType,
        RejectionKind::PayloadTooLarge,
        RejectionKind::BodyRead,
        RejectionKind::MalformedBody,
        RejectionKind::InvalidBody,
    ]
}

impl RequestBodySchema for Bytes {
    fn content_type() -> &'static str {
        OCTET_STREAM
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        vec![RejectionKind::PayloadTooLarge, RejectionKind::BodyRead]
    }
}

impl RequestBodySchema for String {
    fn content_type() -> &'static str {
        "text/plain"
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        vec![
            RejectionKind::PayloadTooLarge,
            RejectionKind::BodyRead,
            RejectionKind::MalformedBody,
        ]
    }
}

/// `application/x-www-form-urlencoded` on non-GET routes (on GET, axum's
/// `Form` reads the query string and the routes macro does not probe it).
/// The form fields are not schematized.
impl<T> RequestBodySchema for Form<T> {
    fn content_type() -> &'static str {
        "application/x-www-form-urlencoded"
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        vec![
            RejectionKind::UnsupportedMediaType,
            RejectionKind::PayloadTooLarge,
            RejectionKind::BodyRead,
            RejectionKind::InvalidBody,
        ]
    }
}

#[cfg(feature = "multipart")]
impl RequestBodySchema for crate::web::multipart::Multipart {
    fn content_type() -> &'static str {
        "multipart/form-data"
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        multipart_kinds()
    }
}

#[cfg(feature = "multipart")]
impl<T: crate::di::meta::MultipartSchema> RequestBodySchema
    for crate::web::multipart::TypedMultipart<T>
{
    fn content_type() -> &'static str {
        "multipart/form-data"
    }
    fn body_schema() -> Option<(String, serde_json::Value)> {
        Some((T::schema_name().to_owned(), T::multipart_schema()))
    }
    fn rejection_kinds() -> Vec<RejectionKind> {
        let mut kinds = multipart_kinds();
        kinds.push(RejectionKind::InvalidBody);
        kinds
    }
}

#[cfg(feature = "multipart")]
fn multipart_kinds() -> Vec<RejectionKind> {
    vec![
        RejectionKind::UnsupportedMediaType,
        RejectionKind::PayloadTooLarge,
        RejectionKind::MalformedBody,
    ]
}

// ── Response bodies ──────────────────────────────────────────────────────────

#[cfg(feature = "openapi")]
impl<T: schemars::JsonSchema> ResponseBodySchema for Json<T> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::json(Some(crate::di::meta::schema_of::<T>()))]
    }
}

#[cfg(not(feature = "openapi"))]
impl<T> ResponseBodySchema for Json<T> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::json(None)]
    }
}

impl ResponseBodySchema for String {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::text()]
    }
}

impl ResponseBodySchema for &'static str {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::text()]
    }
}

impl ResponseBodySchema for Cow<'static, str> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::text()]
    }
}

impl<T> ResponseBodySchema for Html<T> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::html()]
    }
}

impl ResponseBodySchema for Bytes {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::binary()]
    }
}

impl ResponseBodySchema for Vec<u8> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::binary()]
    }
}

/// No body: the route documents its status alone.
impl ResponseBodySchema for () {
    fn response_contents() -> Vec<ResponseContent> {
        Vec::new()
    }
}

impl ResponseBodySchema for StatusCode {
    fn response_contents() -> Vec<ResponseContent> {
        Vec::new()
    }
}

impl ResponseBodySchema for Redirect {
    fn response_contents() -> Vec<ResponseContent> {
        Vec::new()
    }
}

/// The stream item type is opaque at this level; a custom type wrapping the
/// stream documents the event payload through [`ResponseContent::event_stream`].
impl<S> ResponseBodySchema for Sse<S> {
    fn response_contents() -> Vec<ResponseContent> {
        vec![ResponseContent::event_stream(None)]
    }
}

/// `Result<T, E>` (and every alias: `ApiResult<T>`, `StatusResult`, …)
/// documents its `Ok` body; the error side is documented through the route's
/// envelope.
impl<T: ResponseBodySchema, E> ResponseBodySchema for Result<T, E> {
    fn response_contents() -> Vec<ResponseContent> {
        T::response_contents()
    }
}

/// `(StatusCode, T)` / `(HeaderMap, T)` / `(StatusCode, HeaderMap, T)`: the
/// body is the last element; the status is read from the route attribute.
impl<T: ResponseBodySchema> ResponseBodySchema for (StatusCode, T) {
    fn response_contents() -> Vec<ResponseContent> {
        T::response_contents()
    }
}

impl<T: ResponseBodySchema> ResponseBodySchema for (HeaderMap, T) {
    fn response_contents() -> Vec<ResponseContent> {
        T::response_contents()
    }
}

impl<T: ResponseBodySchema> ResponseBodySchema for (StatusCode, HeaderMap, T) {
    fn response_contents() -> Vec<ResponseContent> {
        T::response_contents()
    }
}
