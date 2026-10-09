//! The framework-level responders (#1072 P4): the router's 404 and 405, and
//! the body-limit 413, all answer in the application's error envelope —
//! `HttpError` by default, the `error_projection::<E>()` bean otherwise — and
//! a fallback the app installed itself keeps winning.

use http_body_util::BodyExt;
use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::header::HeaderValue;
use r2e_core::http::response::{IntoHttpResponse, IntoResponse};
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Json, Response, Router, StatusCode, CONTENT_TYPE};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;

use crate::support::raw;

/// A foreign wire shape: `{"error": {"type": .., "message": ..}}` plus a
/// marker header, with `MethodNotAllowed` remapped from 405 to 400 to show
/// `status_of` governs the framework responders too.
#[derive(Debug)]
struct Wire {
    kind: String,
    status: StatusCode,
    message: String,
}

impl From<Rejection> for Wire {
    fn from(r: Rejection) -> Self {
        Self {
            kind: format!("{:?}", r.kind),
            status: r.status,
            message: r.message.into_owned(),
        }
    }
}

impl IntoHttpResponse for Wire {
    fn into_http_response(self) -> Response {
        let body = serde_json::json!({
            "error": { "type": self.kind, "message": self.message }
        });
        let mut resp = (self.status, Json(body)).into_response();
        resp.headers_mut()
            .insert("x-envelope", HeaderValue::from_static("wire"));
        resp
    }
}

r2e_core::http::impl_into_response!(Wire);

impl ErrorSchema for Wire {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::MethodNotAllowed => StatusCode::BAD_REQUEST,
            k => k.default_status(),
        }
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        None
    }
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct Payload {
    name: String,
}

/// `POST /items`: a JSON body on an infallible handler, so its extraction
/// failures (the 413 among them) go through the application projection.
#[controller(path = "/items")]
struct ItemsController;

#[routes]
impl ItemsController {
    #[post("/")]
    async fn create(&self, Json(_body): Json<Payload>) -> &'static str {
        "created"
    }
}

/// `GET /ok`: a raw route, the 405 probe.
fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/ok", get(|| async { "ok" }))
}

async fn default_app() -> Router {
    AppBuilder::new()
        .build_state()
        .await
        .register_controller::<ItemsController>()
        .merge_router(routes())
        .build()
}

async fn wire_app() -> Router {
    AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .register_controller::<ItemsController>()
        .merge_router(routes())
        .build()
}

async fn json_body(resp: Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
    })
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

fn oversized_json() -> Body {
    // Over axum's default 2 MiB body limit.
    let mut s = String::from(r#"{"name":""#);
    s.push_str(&"x".repeat(3 * 1024 * 1024));
    s.push_str(r#""}"#);
    Body::from(s)
}

// ── Default envelope: `HttpError` ──────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn unknown_route_answers_a_json_404_by_default() {
    let resp = raw(default_app().await, "GET", "/missing", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(header(&resp, CONTENT_TYPE.as_str()), Some("application/json"));
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "error": "Not found" })
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn wrong_method_answers_a_json_405_with_allow_by_default() {
    let resp = raw(default_app().await, "DELETE", "/ok", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&resp, CONTENT_TYPE.as_str()), Some("application/json"));
    let allow = header(&resp, "allow").expect("axum keeps the Allow header");
    assert!(allow.contains("GET"), "Allow: {allow}");
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "error": "Method not allowed" })
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn oversized_body_answers_a_json_413_by_default() {
    let resp = raw(
        default_app().await,
        "POST",
        "/items",
        &[("content-type", "application/json")],
        oversized_json(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(header(&resp, CONTENT_TYPE.as_str()), Some("application/json"));
    let body = json_body(resp).await;
    assert!(body.get("error").is_some(), "HttpError envelope: {body}");
}

#[r2e_core::test(flavor = "current_thread")]
async fn routed_requests_are_untouched() {
    let resp = raw(default_app().await, "GET", "/ok", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = raw(
        default_app().await,
        "POST",
        "/items",
        &[("content-type", "application/json")],
        Body::from(r#"{"name":"a"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Application envelope: `error_projection::<Wire>()` ─────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn unknown_route_answers_in_the_app_envelope() {
    let resp = raw(wire_app().await, "GET", "/missing", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(header(&resp, "x-envelope"), Some("wire"));
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "error": { "type": "NotFound", "message": "Not found" } })
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn wrong_method_answers_in_the_app_envelope_with_its_status() {
    let resp = raw(wire_app().await, "DELETE", "/ok", &[], Body::empty()).await;
    // `Wire::status_of(MethodNotAllowed)` remaps 405 to 400.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(header(&resp, "x-envelope"), Some("wire"));
    assert!(header(&resp, "allow").is_some_and(|a| a.contains("GET")));
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({
            "error": { "type": "MethodNotAllowed", "message": "Method not allowed" }
        })
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn oversized_body_answers_in_the_app_envelope() {
    let resp = raw(
        wire_app().await,
        "POST",
        "/items",
        &[("content-type", "application/json")],
        oversized_json(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(header(&resp, "x-envelope"), Some("wire"));
    assert_eq!(
        json_body(resp).await["error"]["type"],
        serde_json::json!("PayloadTooLarge")
    );
}

// ── A fallback the app installed keeps winning ─────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn a_merged_custom_fallback_is_not_overridden() {
    let app = AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .merge_router(routes())
        .merge_router(Router::new().fallback(|| async { (StatusCode::IM_A_TEAPOT, "mine") }))
        .build();

    let resp = raw(app, "GET", "/missing", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(header(&resp, "x-envelope"), None);

    // The 405 responder is per route and independent of the router fallback.
    let app = AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .merge_router(routes())
        .merge_router(Router::new().fallback(|| async { (StatusCode::IM_A_TEAPOT, "mine") }))
        .build();
    let resp = raw(app, "DELETE", "/ok", &[], Body::empty()).await;
    assert_eq!(header(&resp, "x-envelope"), Some("wire"));
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_route_level_method_fallback_is_not_overridden() {
    let app = AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .merge_router(Router::new().route(
            "/own",
            get(|| async { "own" }).fallback(|| async { (StatusCode::IM_A_TEAPOT, "mine") }),
        ))
        .build();

    let resp = raw(app, "DELETE", "/own", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(header(&resp, "x-envelope"), None);
}
