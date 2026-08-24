//! R2E's own `Json<T>` (`r2e-http/src/json.rs`, re-exported as
//! `r2e_core::http::Json`) and the codec façade behind it.
//!
//! Pins the extractor's status policy — the part users observe and the part
//! that must not drift when the codec backend is swapped (`json-sonic`):
//! 415 without a JSON content type, 400 on malformed JSON, 422 on
//! well-formed JSON of the wrong shape, `Option<Json<T>>` → `None` when the
//! body is absent, and `application/json` on the response.

use r2e_core::http::routing::post;
use r2e_core::http::{Body, Json, Router, StatusCode};
use serde::{Deserialize, Serialize};

use crate::support::{raw, send};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Payload {
    name: String,
    count: u32,
}

async fn echo(Json(p): Json<Payload>) -> Json<Payload> {
    Json(p)
}

async fn optional(body: Option<Json<Payload>>) -> String {
    match body {
        Some(Json(p)) => p.name,
        None => "none".to_string(),
    }
}

fn router() -> Router {
    Router::new()
        .route("/echo", post(echo))
        .route("/optional", post(optional))
}

const JSON: (&str, &str) = ("content-type", "application/json");

#[r2e_core::test]
async fn missing_content_type_is_415() {
    let (status, body) = send(
        router(),
        "POST",
        "/echo",
        &[],
        Body::from(r#"{"name":"a","count":1}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(body.contains("application/json"), "body: {body}");
}

#[r2e_core::test]
async fn syntax_error_is_400() {
    let (status, _) = send(
        router(),
        "POST",
        "/echo",
        &[JSON],
        Body::from(r#"{"name":"a", "#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[r2e_core::test]
async fn data_error_is_422() {
    // Well-formed JSON, wrong shape: `count` is a string.
    let (status, _) = send(
        router(),
        "POST",
        "/echo",
        &[JSON],
        Body::from(r#"{"name":"a","count":"nope"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[r2e_core::test]
async fn optional_json_is_none_without_content_type() {
    let (status, body) = send(router(), "POST", "/optional", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "none");

    let (status, body) = send(
        router(),
        "POST",
        "/optional",
        &[JSON],
        Body::from(r#"{"name":"here","count":2}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "here");
}

#[r2e_core::test]
async fn response_carries_application_json() {
    let resp = raw(
        router(),
        "POST",
        "/echo",
        &[JSON],
        Body::from(r#"{"name":"a","count":1}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/json"
    );
    let body = crate::support::body_string(resp).await;
    let round: Payload = r2e_core::json::from_str(&body).unwrap();
    assert_eq!(
        round,
        Payload {
            name: "a".into(),
            count: 1
        }
    );
}

#[r2e_core::test]
async fn facade_classifies_syntax_versus_data() {
    let data = r2e_core::json::from_str::<Payload>(r#"{"name":"a","count":"nope"}"#).unwrap_err();
    assert!(data.is_data(), "expected Data, got {:?}", data.kind());

    let syntax = r2e_core::json::from_str::<Payload>("{").unwrap_err();
    assert!(
        !syntax.is_data(),
        "expected non-Data, got {:?}",
        syntax.kind()
    );
}
