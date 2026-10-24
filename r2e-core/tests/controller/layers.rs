//! Controller-level `#[middleware]` / `#[layer]` (#1074): impl-level tower
//! layers wrap every route of the controller (outside the method-level ones),
//! `#[middleware(fn, state)]` hands the function the controller core, and the
//! `#[layer(|this: &Self| ..)]` closure builds a layer from the core once, at
//! route registration.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use http_body_util::BodyExt;
use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::extract::State;
use r2e_core::http::header::HeaderValue;
use r2e_core::http::middleware::Next;
use r2e_core::http::response::{IntoHttpResponse, IntoResponse, Response};
use r2e_core::http::{Body, DefaultBodyLimit, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::type_list::HNil;
use r2e_core::AppBuilder;
use serde::Deserialize;
use tower::ServiceExt;

// ── Helpers ────────────────────────────────────────────────────────────────

/// Append `value` to the response's `x-trace` header. Middlewares append on
/// the way out, so the innermost one appends first.
fn trace(mut resp: Response, value: &'static str) -> Response {
    resp.headers_mut()
        .append("x-trace", HeaderValue::from_static(value));
    resp
}

fn traces(resp: &Response) -> Vec<String> {
    resp.headers()
        .get_all("x-trace")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

async fn ctrl_mw(req: Request, next: Next) -> Response {
    trace(next.run(req).await, "ctrl")
}

async fn method_mw(req: Request, next: Next) -> Response {
    trace(next.run(req).await, "method")
}

async fn mw_a(req: Request, next: Next) -> Response {
    trace(next.run(req).await, "a")
}

async fn mw_b(req: Request, next: Next) -> Response {
    trace(next.run(req).await, "b")
}

async fn send(router: r2e_core::http::Router, req: Request) -> Response {
    router.oneshot(req).await.unwrap()
}

fn get(path: &str) -> Request {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── (a) (b) (f) Impl-level middleware reaches every route, outside ────────

#[controller(path = "/layered")]
pub struct LayeredController;

#[routes]
#[middleware(ctrl_mw)]
impl LayeredController {
    /// No method-level attribute: only the controller's middleware.
    #[get("/plain")]
    async fn plain(&self) -> &'static str {
        "plain"
    }

    /// Method-level middleware: wrapped by the controller's.
    #[get("/nested")]
    #[middleware(method_mw)]
    async fn nested(&self) -> &'static str {
        "nested"
    }

    /// Two method-level middlewares: the later one is the outer one.
    #[get("/two")]
    #[middleware(mw_a)]
    #[middleware(mw_b)]
    async fn two(&self) -> &'static str {
        "two"
    }

    #[sse("/events")]
    async fn events(
        &self,
    ) -> impl futures_core::Stream<
        Item = Result<r2e_core::http::response::SseEvent, std::convert::Infallible>,
    > {
        use r2e_core::rt::stream::wrappers::ReceiverStream;
        let (tx, rx) = r2e_core::rt::sync::mpsc::channel(1);
        tx.send(Ok(
            r2e_core::http::response::SseEvent::default().data("hello")
        ))
        .await
        .unwrap();
        drop(tx);
        ReceiverStream::new(rx)
    }
}

async fn layered_router() -> r2e_core::http::Router {
    AppBuilder::new()
        .build_state()
        .await
        .register_controller::<LayeredController>()
        .build()
}

#[r2e_core::test]
async fn impl_level_middleware_reaches_a_route_without_method_attrs() {
    let resp = send(layered_router().await, get("/layered/plain")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(traces(&resp), ["ctrl"]);
    assert_eq!(body_string(resp).await, "plain");
}

#[r2e_core::test]
async fn controller_middleware_wraps_method_middleware() {
    let resp = send(layered_router().await, get("/layered/nested")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Inner appends first on the way out: method is inside, ctrl outside.
    assert_eq!(traces(&resp), ["method", "ctrl"]);
}

#[r2e_core::test]
async fn later_declared_method_middleware_is_outer() {
    let resp = send(layered_router().await, get("/layered/two")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(traces(&resp), ["a", "b", "ctrl"]);
}

#[r2e_core::test]
async fn impl_level_middleware_reaches_sse_routes() {
    let resp = send(layered_router().await, get("/layered/events")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(traces(&resp), ["ctrl"]);
    assert!(body_string(resp).await.contains("hello"));
}

// ── (c) `#[middleware(fn, state)]` reads the controller core ──────────────

#[derive(Clone)]
pub struct Greeting(&'static str);

#[controller(path = "/stateful")]
pub struct StatefulController {
    #[inject]
    greeting: Greeting,
    #[config("app.tag")]
    tag: String,
}

async fn core_mw(
    State(ctrl): State<Arc<StatefulController>>,
    req: Request,
    next: Next,
) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("x-greeting", HeaderValue::from_static(ctrl.greeting.0));
    resp.headers_mut()
        .insert("x-tag", HeaderValue::from_str(&ctrl.tag).unwrap());
    resp
}

async fn method_core_mw(
    State(ctrl): State<Arc<StatefulController>>,
    req: Request,
    next: Next,
) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        "x-method-tag",
        HeaderValue::from_str(&format!("{}!", ctrl.tag)).unwrap(),
    );
    resp
}

#[routes]
#[middleware(core_mw, state)]
impl StatefulController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "ok"
    }

    #[get("/method")]
    #[middleware(method_core_mw, state)]
    async fn method(&self) -> &'static str {
        "ok"
    }
}

#[r2e_core::test]
async fn state_middleware_reads_a_bean_and_a_config_value_from_the_core() {
    let mut config = R2eConfig::empty();
    config.set("app.tag", ConfigValue::String("blue".into()));
    let router = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .provide(Greeting("hello"))
        .build_state()
        .await
        .register_controller::<StatefulController>()
        .build();

    let resp = send(router.clone(), get("/stateful")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-greeting"], "hello");
    assert_eq!(resp.headers()["x-tag"], "blue");

    let resp = send(router, get("/stateful/method")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-method-tag"], "blue!");
    assert_eq!(resp.headers()["x-tag"], "blue");
}

// ── (d) The `|this: &Self|` closure runs once, at registration ────────────

#[derive(Clone, Default)]
pub struct LayerBuilds {
    ctrl: Arc<AtomicUsize>,
    method: Arc<AtomicUsize>,
}

#[controller(path = "/once")]
pub struct OnceController {
    #[inject]
    builds: LayerBuilds,
}

#[routes]
#[layer(|this: &Self| {
    this.builds.ctrl.fetch_add(1, Ordering::SeqCst);
    DefaultBodyLimit::max(1 << 20)
})]
impl OnceController {
    #[get("/a")]
    async fn a(&self) -> &'static str {
        "a"
    }

    #[get("/b")]
    #[layer(|this| {
        this.builds.method.fetch_add(1, Ordering::SeqCst);
        DefaultBodyLimit::max(1 << 20)
    })]
    async fn b(&self) -> &'static str {
        "b"
    }
}

#[r2e_core::test]
async fn layer_closure_is_evaluated_once_at_registration() {
    let builds = LayerBuilds::default();
    let router = AppBuilder::new()
        .provide(builds.clone())
        .build_state()
        .await
        .register_controller::<OnceController>()
        .build();
    // Built before any request: the controller-level layer once for the
    // whole controller (cloned onto both routes), the method-level one once.
    assert_eq!(builds.ctrl.load(Ordering::SeqCst), 1);
    assert_eq!(builds.method.load(Ordering::SeqCst), 1);

    for path in ["/once/a", "/once/b", "/once/a", "/once/b"] {
        assert_eq!(
            send(router.clone(), get(path)).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(builds.ctrl.load(Ordering::SeqCst), 1);
    assert_eq!(builds.method.load(Ordering::SeqCst), 1);
}

// ── (e) Controller-level body limit → 413 through the route's envelope ────

/// Route envelope: marks its responses so the test can tell the projection
/// went through it.
#[derive(Debug)]
pub struct Wire(Rejection);

impl From<Rejection> for Wire {
    fn from(r: Rejection) -> Self {
        Self(r)
    }
}

impl IntoHttpResponse for Wire {
    fn into_http_response(self) -> Response {
        let kind = format!("{:?}", self.0.kind);
        let mut resp = (self.0.status, kind).into_response();
        resp.headers_mut()
            .insert("x-envelope", HeaderValue::from_static("wire"));
        resp
    }
}

r2e_core::http::impl_into_response!(Wire);

impl ErrorSchema for Wire {
    fn body_schema() -> Option<(String, serde_json::Value)> {
        None
    }
}

#[derive(Deserialize)]
pub struct Payload {
    text: String,
}

#[controller(path = "/proxy")]
pub struct ProxyController {
    #[config("proxy.request-body-limit-bytes")]
    body_limit: usize,
}

#[routes]
#[layer(|this: &Self| DefaultBodyLimit::max(this.body_limit))]
impl ProxyController {
    #[post("/upload")]
    async fn upload(&self, Json(payload): Json<Payload>) -> Result<String, Wire> {
        Ok(payload.text)
    }
}

fn post_json(path: &str, body: String) -> Request {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

#[r2e_core::test]
async fn controller_level_body_limit_answers_413_through_the_route_envelope() {
    let mut config = R2eConfig::empty();
    config.set("proxy.request-body-limit-bytes", ConfigValue::Integer(32));
    let router = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await
        .register_controller::<ProxyController>()
        .build();

    let small = send(
        router.clone(),
        post_json("/proxy/upload", r#"{"text":"hi"}"#.into()),
    )
    .await;
    assert_eq!(small.status(), StatusCode::OK);
    assert_eq!(body_string(small).await, "hi");

    let big = format!(r#"{{"text":"{}"}}"#, "x".repeat(256));
    let resp = send(router, post_json("/proxy/upload", big)).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(resp.headers()["x-envelope"], "wire");
    assert_eq!(body_string(resp).await, "PayloadTooLarge");
}

#[test]
fn a_route_with_a_body_documents_payload_too_large() {
    let mut registry = MetaRegistry::new();
    <ProxyController as Controller<HNil, _>>::register_meta(&mut registry);
    let routes = registry.take::<RouteInfo>();
    let upload = routes
        .iter()
        .find(|r| r.path == "/proxy/upload")
        .expect("upload route");
    assert!(
        upload
            .rejection_kinds
            .contains(&RejectionKind::PayloadTooLarge),
        "kinds: {:?}",
        upload.rejection_kinds
    );
}
