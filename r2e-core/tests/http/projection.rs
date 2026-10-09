//! The single projection point (#1072 P1).
//!
//! Every failure of a request — pre-auth guards, identity, guards, path and
//! body extraction, garde validation, managed acquire/finalize, the handler's
//! own `Err` — becomes a `Rejection` and is rendered **once**, through the
//! envelope the route declares in its return type: `Result<T, E>` with
//! `E: From<Rejection> + IntoHttpResponse + ErrorSchema`. A route without such
//! an envelope (infallible, or an error type that is not one) falls back to
//! the application projection (`AppBuilder::error_projection::<E>()`), else
//! `HttpError`.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use http_body_util::BodyExt;
use r2e_core::decorators::guards::PreAuthGuardContext;
use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::Bytes;
use r2e_core::BeanAccess;
use r2e_core::http::extract::{FromRequestParts, Path};
use r2e_core::http::header::{HeaderValue, Parts, RETRY_AFTER};
use r2e_core::http::response::{IntoHttpResponse, IntoResponse};
use r2e_core::http::{Body, Json, Response, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::web::managed::ManagedErr;
use r2e_core::{
    Guard, GuardContext, HttpError, Identity, ManagedContext, ManagedDeps, ManagedOutcome,
    ManagedResource, PreAuthGuard, TNil,
};

use crate::support::raw;

// ── Envelopes ──────────────────────────────────────────────────────────────

/// A foreign wire shape: `{"error": {"type": .., "message": ..}}` plus a
/// marker header, and `InvalidBody` remapped from 422 to 400.
#[derive(Debug)]
struct Wire {
    kind: String,
    status: StatusCode,
    message: String,
    details: Option<serde_json::Value>,
}

impl From<Rejection> for Wire {
    fn from(r: Rejection) -> Self {
        Self {
            kind: format!("{:?}", r.kind),
            status: r.status,
            message: r.message.into_owned(),
            details: r.details,
        }
    }
}

impl IntoHttpResponse for Wire {
    fn into_http_response(self) -> Response {
        let body = serde_json::json!({
            "error": { "type": self.kind, "message": self.message, "details": self.details }
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
            RejectionKind::InvalidBody => StatusCode::BAD_REQUEST,
            k => k.default_status(),
        }
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        None
    }
}

/// The application-level envelope: the `HttpError` body plus a marker header.
#[derive(Debug)]
struct AppWire(HttpError);

impl From<Rejection> for AppWire {
    fn from(r: Rejection) -> Self {
        Self(HttpError::from(r))
    }
}

impl IntoHttpResponse for AppWire {
    fn into_http_response(self) -> Response {
        let mut resp = self.0.into_http_response();
        resp.headers_mut()
            .insert("x-envelope", HeaderValue::from_static("app"));
        resp
    }
}

r2e_core::http::impl_into_response!(AppWire);

impl ErrorSchema for AppWire {
    fn body_schema() -> Option<(String, serde_json::Value)> {
        HttpError::body_schema()
    }
}

/// An error type that renders but is **not** an envelope (no
/// `From<Rejection>`, no `ErrorSchema`): the route keeps the default
/// projection for framework failures.
#[derive(Debug)]
struct PlainErr;

impl IntoHttpResponse for PlainErr {
    fn into_http_response(self) -> Response {
        (StatusCode::IM_A_TEAPOT, "plain").into_response()
    }
}

r2e_core::http::impl_into_response!(PlainErr);

// ── Request-scoped fixtures ────────────────────────────────────────────────

/// Identity from `x-user`; fails with a typed rejection.
struct Subject(String);

impl Identity for Subject {
    fn sub(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Subject {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|sub| Subject(sub.to_owned()))
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, "who are you?"))
    }
}

/// Pre-auth guard: `x-throttle` → 429 with `Retry-After`.
struct Throttle;
impl SelfBuilt for Throttle {}
impl PreAuthGuard for Throttle {
    async fn check(&self, ctx: &PreAuthGuardContext<'_>) -> Result<(), Rejection> {
        if ctx.headers.contains_key("x-throttle") {
            return Err(Rejection::new(RejectionKind::RateLimited, "slow down")
                .header(RETRY_AFTER, HeaderValue::from_static("7")));
        }
        Ok(())
    }
}

/// Post-auth guard: `x-deny` → 403.
struct DenyHeader;
impl SelfBuilt for DenyHeader {}
impl<I: Identity> Guard<I> for DenyHeader {
    async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Rejection> {
        if ctx.headers.contains_key("x-deny") {
            return Err(Rejection::forbidden("denied by header"));
        }
        Ok(())
    }
}

/// Managed resource failing on demand: `x-fail: acquire` / `x-fail: finalize`.
struct Flaky {
    fail_finalize: bool,
}

impl<S: Send + Sync> ManagedResource<S> for Flaky {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let head = context.require_request()?;
        match head.header("x-fail") {
            Some("acquire") => Err(ManagedErr(HttpError::BadRequest("acquire failed".into()))),
            Some("finalize") => Ok(Self { fail_finalize: true }),
            _ => Ok(Self { fail_finalize: false }),
        }
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        if self.fail_finalize {
            return Err(ManagedErr(HttpError::internal("finalize failed")));
        }
        Ok(())
    }

    fn abort(&mut self) {}
}

impl ManagedDeps for Flaky {
    type Deps = TNil;
}

#[derive(serde::Deserialize, garde::Validate)]
struct Payload {
    #[garde(length(min = 1))]
    name: String,
}

// ── Controller ─────────────────────────────────────────────────────────────

#[controller(path = "/proj")]
struct ProjController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl ProjController {
    /// Every door on one route: pre-guard, identity, guard, path, body,
    /// validation, handler `Err`.
    #[post("/items/{id}")]
    #[pre_guard(Throttle)]
    #[guard(DenyHeader)]
    async fn create(&self, Path(id): Path<u32>, Json(body): Json<Payload>) -> Result<String, Wire> {
        if body.name == "boom" {
            return Err(Wire {
                kind: "Handler".into(),
                status: StatusCode::IM_A_TEAPOT,
                message: "handler said no".into(),
                details: None,
            });
        }
        Ok(format!("{}:{}:{}", self.user.0, id, body.name))
    }

    #[get("/managed")]
    async fn managed(&self, #[managed] _res: &mut Flaky) -> Result<&'static str, Wire> {
        Ok("managed-ok")
    }

    /// `PlainErr` renders but is no envelope → default projection.
    #[get("/plain")]
    async fn plain(&self) -> Result<&'static str, PlainErr> {
        Ok("plain")
    }

    /// Declared infallible → default projection.
    #[get("/infallible")]
    async fn infallible(&self) -> &'static str {
        "infallible"
    }

    /// `#[anonymous]` routes still project through their envelope.
    #[get("/anon/{id}")]
    #[anonymous]
    async fn anon(&self, Path(id): Path<u32>) -> Result<String, Wire> {
        Ok(id.to_string())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

async fn router() -> Router {
    r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<ProjController>()
        .build()
}

async fn router_with_app_envelope() -> Router {
    r2e_core::AppBuilder::new()
        .error_projection::<AppWire>()
        .build_state()
        .await
        .register_controller::<ProjController>()
        .build()
}

async fn json_body(resp: Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
    })
}

/// `POST /proj/items/{id}` as `zoe` with JSON content-type, plus extra headers.
async fn post(path: &str, extra: &[(&str, &str)], body: &'static str) -> Response {
    let mut headers = vec![("x-user", "zoe"), ("content-type", "application/json")];
    headers.extend_from_slice(extra);
    raw(router().await, "POST", path, &headers, Body::from(body)).await
}

fn envelope(resp: &Response) -> Option<&str> {
    resp.headers().get("x-envelope").and_then(|v| v.to_str().ok())
}

/// Asserts a `Wire` envelope: marker header, status, `error.type`.
async fn assert_wire(resp: Response, status: StatusCode, kind: &str) -> serde_json::Value {
    assert_eq!(envelope(&resp), Some("wire"), "expected the route envelope");
    assert_eq!(resp.status(), status);
    let json = json_body(resp).await;
    assert_eq!(json["error"]["type"], kind, "{json}");
    json
}

// ── Return-type envelope: every door ───────────────────────────────────────

#[r2e_core::test]
async fn success_path_is_untouched() {
    let resp = post("/proj/items/7", &[], r#"{"name":"ok"}"#).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(envelope(&resp), None);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"zoe:7:ok");
}

#[r2e_core::test]
async fn pre_auth_guard_projects_before_identity() {
    // No `x-user` AND `x-throttle`: the pre-auth guard answers first.
    let router = router().await;
    let resp = raw(
        router,
        "POST",
        "/proj/items/7",
        &[("x-throttle", "1"), ("content-type", "application/json")],
        Body::from(r#"{"name":"ok"}"#),
    )
    .await;
    assert_eq!(resp.headers()[RETRY_AFTER], "7", "hub headers reach the wire");
    let json = assert_wire(resp, StatusCode::TOO_MANY_REQUESTS, "RateLimited").await;
    assert_eq!(json["error"]["message"], "slow down");
}

#[r2e_core::test]
async fn identity_failure_projects() {
    let router = router().await;
    let resp = raw(
        router,
        "POST",
        "/proj/items/7",
        &[("content-type", "application/json")],
        Body::from(r#"{"name":"ok"}"#),
    )
    .await;
    let json = assert_wire(resp, StatusCode::UNAUTHORIZED, "Unauthenticated").await;
    assert_eq!(json["error"]["message"], "who are you?");
}

#[r2e_core::test]
async fn guard_failure_projects() {
    let resp = post("/proj/items/7", &[("x-deny", "1")], r#"{"name":"ok"}"#).await;
    assert_wire(resp, StatusCode::FORBIDDEN, "Forbidden").await;
}

#[r2e_core::test]
async fn bad_path_projects() {
    let resp = post("/proj/items/abc", &[], r#"{"name":"ok"}"#).await;
    assert_wire(resp, StatusCode::BAD_REQUEST, "InvalidPath").await;
}

#[r2e_core::test]
async fn missing_content_type_projects() {
    let router = router().await;
    let resp = raw(
        router,
        "POST",
        "/proj/items/7",
        &[("x-user", "zoe")],
        Body::from(r#"{"name":"ok"}"#),
    )
    .await;
    assert_wire(resp, StatusCode::UNSUPPORTED_MEDIA_TYPE, "MissingContentType").await;
}

#[r2e_core::test]
async fn malformed_body_projects() {
    let resp = post("/proj/items/7", &[], r#"{bad"#).await;
    assert_wire(resp, StatusCode::BAD_REQUEST, "MalformedBody").await;
}

#[r2e_core::test]
async fn wrong_shape_body_projects_with_the_envelope_status() {
    // `InvalidBody` defaults to 422; `Wire::status_of` remaps it to 400 and the
    // runtime applies the remap before `From<Rejection>`.
    assert_eq!(
        RejectionKind::InvalidBody.default_status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let resp = post("/proj/items/7", &[], r#"{"nam":"ok"}"#).await;
    assert_wire(resp, StatusCode::BAD_REQUEST, "InvalidBody").await;
}

#[r2e_core::test]
async fn validation_failure_projects() {
    let resp = post("/proj/items/7", &[], r#"{"name":""}"#).await;
    let json = assert_wire(resp, StatusCode::BAD_REQUEST, "Validation").await;
    // The garde report travels in `details`, byte-equal to what `HttpError`
    // renders; the message stays the fixed "Validation failed".
    assert_eq!(json["error"]["message"], "Validation failed");
    assert!(
        json["error"]["details"].to_string().contains("name"),
        "{json}"
    );
}

#[r2e_core::test]
async fn handler_error_renders_through_the_same_envelope() {
    let resp = post("/proj/items/7", &[], r#"{"name":"boom"}"#).await;
    let json = assert_wire(resp, StatusCode::IM_A_TEAPOT, "Handler").await;
    assert_eq!(json["error"]["message"], "handler said no");
}

#[r2e_core::test]
async fn managed_acquire_and_finalize_project() {
    let router = router().await;

    let resp = raw(
        router.clone(),
        "GET",
        "/proj/managed",
        &[("x-user", "zoe"), ("x-fail", "acquire")],
        Body::empty(),
    )
    .await;
    let json = assert_wire(resp, StatusCode::BAD_REQUEST, "BadRequest").await;
    assert_eq!(json["error"]["message"], "acquire failed");

    let resp = raw(
        router.clone(),
        "GET",
        "/proj/managed",
        &[("x-user", "zoe"), ("x-fail", "finalize")],
        Body::empty(),
    )
    .await;
    let json = assert_wire(resp, StatusCode::INTERNAL_SERVER_ERROR, "Internal").await;
    assert_eq!(json["error"]["message"], "finalize failed");

    let resp = raw(router, "GET", "/proj/managed", &[("x-user", "zoe")], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[r2e_core::test]
async fn anonymous_route_projects_through_its_envelope() {
    let resp = raw(router().await, "GET", "/proj/anon/abc", &[], Body::empty()).await;
    assert_wire(resp, StatusCode::BAD_REQUEST, "InvalidPath").await;
}

// ── Body is not read before identity and guards ────────────────────────────

/// A body whose first poll is counted, so a test can prove it never happened.
fn counting_body() -> (Body, Arc<AtomicUsize>) {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);
    let stream = futures_util::stream::once(async move {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok::<Bytes, Infallible>(Bytes::from_static(br#"{"name":"ok"}"#))
    });
    (Body::from_stream(stream), polls)
}

#[r2e_core::test]
async fn identity_failure_never_reads_the_body() {
    let (body, polls) = counting_body();
    let resp = raw(
        router().await,
        "POST",
        "/proj/items/7",
        &[("content-type", "application/json")],
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(polls.load(Ordering::SeqCst), 0, "body polled before identity");
}

#[r2e_core::test]
async fn guard_failure_never_reads_the_body() {
    let (body, polls) = counting_body();
    let resp = raw(
        router().await,
        "POST",
        "/proj/items/7",
        &[
            ("x-user", "zoe"),
            ("x-deny", "1"),
            ("content-type", "application/json"),
        ],
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(polls.load(Ordering::SeqCst), 0, "body polled before guards");
}

#[r2e_core::test]
async fn success_reads_the_body_once() {
    let (body, polls) = counting_body();
    let resp = raw(
        router().await,
        "POST",
        "/proj/items/7",
        &[("x-user", "zoe"), ("content-type", "application/json")],
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(polls.load(Ordering::SeqCst), 1);
}

// ── No envelope: default projection ────────────────────────────────────────

#[r2e_core::test]
async fn plain_error_type_falls_back_to_http_error() {
    let resp = raw(router().await, "GET", "/proj/plain", &[], Body::empty()).await;
    assert_eq!(envelope(&resp), None, "PlainErr is not an envelope");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let json = json_body(resp).await;
    assert_eq!(json["error"], "who are you?", "{json}");
}

#[r2e_core::test]
async fn infallible_route_falls_back_to_http_error() {
    let resp = raw(router().await, "GET", "/proj/infallible", &[], Body::empty()).await;
    assert_eq!(envelope(&resp), None);
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let json = json_body(resp).await;
    assert_eq!(json["error"], "who are you?", "{json}");
}

// ── App-level envelope ─────────────────────────────────────────────────────

#[r2e_core::test]
async fn app_level_envelope_covers_routes_without_one() {
    let router = router_with_app_envelope().await;

    for path in ["/proj/plain", "/proj/infallible"] {
        let resp = raw(router.clone(), "GET", path, &[], Body::empty()).await;
        assert_eq!(envelope(&resp), Some("app"), "{path}");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let json = json_body(resp).await;
        assert_eq!(json["error"], "who are you?", "{json}");
    }
}

#[r2e_core::test]
async fn return_type_envelope_wins_over_the_app_level_one() {
    let router = router_with_app_envelope().await;
    let resp = raw(
        router,
        "POST",
        "/proj/items/7",
        &[("content-type", "application/json")],
        Body::from(r#"{"name":"ok"}"#),
    )
    .await;
    assert_wire(resp, StatusCode::UNAUTHORIZED, "Unauthenticated").await;
}

#[r2e_core::test]
async fn error_projector_is_an_ordinary_bean() {
    let state = r2e_core::AppBuilder::new()
        .error_projection::<AppWire>()
        .build_state()
        .await;
    let projector = state.state().get::<r2e_core::ErrorProjector>();
    let resp = projector.project(Rejection::not_found("gone"));
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(envelope(&resp), Some("app"));
}
