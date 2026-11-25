//! The JSON codec façade (`r2e_http::json`): error classification, the
//! `Json<T>` content-type gate and its rejections.

use std::error::Error as _;

use r2e_http::extract::{FromRequest, OptionalFromRequest};
use r2e_http::json::{self, JsonErrorKind};
use r2e_http::{Body, Json, JsonError, JsonRejection, Request, StatusCode};
use serde::de::Deserialize;

// ── JsonError classification ───────────────────────────────────────────────

/// A `serde_json` error of a known kind, routed through `From`, so the
/// classification does not depend on the compiled-in backend.
fn serde_error<T: for<'de> Deserialize<'de>>(input: &[u8]) -> JsonError {
    let mut de = serde_json::Deserializer::from_slice(input);
    JsonError::from(T::deserialize(&mut de).err().expect("input must fail"))
}

struct FailingReader;

impl std::io::Read for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk on fire"))
    }
}

fn io_error() -> JsonError {
    let mut de = serde_json::Deserializer::from_reader(FailingReader);
    JsonError::from(u32::deserialize(&mut de).unwrap_err())
}

fn predicates(e: &JsonError) -> [bool; 4] {
    [e.is_data(), e.is_syntax(), e.is_eof(), e.is_io()]
}

#[test]
fn each_kind_sets_exactly_its_predicate() {
    let cases = [
        (
            serde_error::<u32>(br#""text""#),
            JsonErrorKind::Data,
            [true, false, false, false],
        ),
        (
            serde_error::<u32>(b"@oops"),
            JsonErrorKind::Syntax,
            [false, true, false, false],
        ),
        (
            serde_error::<Vec<u32>>(b"[1,"),
            JsonErrorKind::Eof,
            [false, false, true, false],
        ),
        (io_error(), JsonErrorKind::Io, [false, false, false, true]),
    ];
    for (err, kind, expected) in cases {
        assert_eq!(err.kind(), kind, "{err}");
        assert_eq!(predicates(&err), expected, "{kind:?}");
    }
}

#[test]
fn backend_errors_agree_with_their_kind() {
    for input in [&br#""text""#[..], b"@oops", b"[1,"] {
        let err = json::from_slice::<Vec<u32>>(input).unwrap_err();
        let expected = [
            err.kind() == JsonErrorKind::Data,
            err.kind() == JsonErrorKind::Syntax,
            err.kind() == JsonErrorKind::Eof,
            err.kind() == JsonErrorKind::Io,
        ];
        assert_eq!(predicates(&err), expected, "{err}");
    }
    // Wrong shape is `Data` on every backend: it is what makes a 422.
    assert!(json::from_slice::<u32>(br#""text""#).unwrap_err().is_data());
}

#[test]
fn error_source_is_the_backend_error() {
    let err = io_error();
    let source = err.source().expect("JsonError always has a source");
    assert_eq!(source.to_string(), err.to_string());
    assert!(source.downcast_ref::<serde_json::Error>().is_some());
}

#[test]
fn to_vec_and_to_string_serialize_the_value() {
    assert_eq!(json::to_vec(&[1, 2]).unwrap(), b"[1,2]");
    assert_eq!(json::to_string(&"hi").unwrap(), r#""hi""#);
}

// ── Json<T> content-type gate ──────────────────────────────────────────────

fn request(content_type: Option<&str>, body: &'static str) -> Request {
    let mut builder = Request::builder().method("POST").uri("/");
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    builder.body(Body::from(body)).unwrap()
}

async fn extract(content_type: Option<&str>, body: &'static str) -> Result<u32, JsonRejection> {
    <Json<u32> as FromRequest<()>>::from_request(request(content_type, body), &())
        .await
        .map(Json::into_inner)
}

#[tokio::test]
async fn json_content_types_are_accepted() {
    for ct in [
        "application/json",
        "APPLICATION/JSON",
        "application/json; charset=utf-8",
        " application/json ;charset=utf-8",
        "application/merge-patch+json",
        "application/vnd.api+JSON",
        "text/x+json",
    ] {
        assert_eq!(extract(Some(ct), "7").await.unwrap(), 7, "{ct}");
    }
}

#[tokio::test]
async fn non_json_content_types_are_rejected_with_415() {
    for ct in [
        None,
        Some("application/jsonx"),
        Some("text/json"),
        Some("text/plain"),
        Some("application/+json"), // empty subtype prefix: suffix alone is not enough
        Some("application/x"),
        Some("json"),
        Some(""),
    ] {
        let rejection = extract(ct, "7").await.unwrap_err();
        assert!(
            matches!(rejection, JsonRejection::MissingContentType),
            "{ct:?}: {rejection:?}"
        );
        assert_eq!(rejection.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(rejection.source().is_none());
    }
}

#[tokio::test]
async fn optional_json_is_none_without_content_type_and_parsed_with_it() {
    let none = <Json<u32> as OptionalFromRequest<()>>::from_request(request(None, "7"), &())
        .await
        .unwrap();
    assert!(none.is_none());

    let some = <Json<u32> as OptionalFromRequest<()>>::from_request(
        request(Some("application/json"), "7"),
        &(),
    )
    .await
    .unwrap();
    assert_eq!(some.map(Json::into_inner), Some(7));

    // A bad body is still a rejection, not `None`.
    let bad = <Json<u32> as OptionalFromRequest<()>>::from_request(
        request(Some("application/json"), "\"x\""),
        &(),
    )
    .await
    .unwrap_err();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn body_rejections_map_to_status_and_keep_their_source() {
    let data = extract(Some("application/json"), "\"x\"")
        .await
        .unwrap_err();
    assert!(matches!(data, JsonRejection::Data(_)), "{data:?}");
    assert_eq!(data.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(data
        .body_text()
        .starts_with("Failed to deserialize the JSON body"));
    assert!(data.source().is_some());

    let syntax = extract(Some("application/json"), "@oops")
        .await
        .unwrap_err();
    assert_eq!(syntax.status(), StatusCode::BAD_REQUEST);
    assert!(syntax
        .body_text()
        .starts_with("Failed to parse the request body"));
    assert!(syntax.source().is_some());

    let missing = extract(None, "7").await.unwrap_err();
    assert_eq!(
        missing.body_text(),
        "Expected request with `Content-Type: application/json`"
    );
}

// ── responses ──────────────────────────────────────────────────────────────

async fn body_string(resp: r2e_http::response::Response) -> String {
    use http_body_util::BodyExt;
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn json_responses_carry_the_serialized_body() {
    use r2e_http::response::{IntoHttpResponse, IntoResponse};

    for resp in [
        Json([1, 2]).into_http_response(),
        Json([1, 2]).into_response(),
    ] {
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "application/json");
        assert_eq!(body_string(resp).await, "[1,2]");
    }
}

#[tokio::test]
async fn rejection_response_uses_its_status_and_text() {
    use r2e_http::response::IntoHttpResponse;

    let resp = JsonRejection::MissingContentType.into_http_response();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(resp.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        body_string(resp).await,
        "Expected request with `Content-Type: application/json`"
    );
}
