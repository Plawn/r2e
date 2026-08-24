use http_body_util::BodyExt;
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::{IntoResponse, Response, StatusCode};
use r2e_core::prelude::*;
use r2e_core::web::extract::OptionalFromRequestPartsVia;
use r2e_core::web::managed::ManagedErr;
use r2e_core::{
    Guard, GuardContext, HttpError, Identity, InterceptorContext, ManagedContext, ManagedDeps,
    ManagedGuard, ManagedOutcome, ManagedResource, TNil,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::support::send_get_with;

#[r2e_core::test]
async fn managed_err_http_error_into_response() {
    let err = ManagedErr(HttpError::NotFound("gone".into()));
    let resp: Response = err.into();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "gone");
}

#[derive(Debug)]
struct TestError(String);

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl IntoResponse for TestError {
    fn into_response(self) -> Response {
        (StatusCode::CONFLICT, self.0).into_response()
    }
}

#[r2e_core::test]
async fn managed_err_wraps_custom_error() {
    let err = ManagedErr(TestError("conflict!".into()));
    let resp: Response = err.into();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8_lossy(&body), "conflict!");
}

#[test]
fn managed_err_display_delegates() {
    let err = ManagedErr(TestError("hello".into()));
    assert_eq!(err.to_string(), "hello");
    assert_eq!(format!("{:?}", err), "ManagedErr(TestError(\"hello\"))");
}

// ── Request head at acquire time ───────────────────────────────────────────
//
// Generated handlers extract the request head once per request and hand it to
// every `#[managed]` acquisition of the route. The probe below snapshots what
// it sees into a string the handler returns verbatim, so one request asserts
// method, header, and path parameter in one body — across every handler shape
// the codegen emits (plain, guarded, intercepted, and `#[anonymous]` on an
// identity controller).

#[derive(Debug)]
struct HeadProbe {
    summary: String,
}

impl<S: Send + Sync> ManagedResource<S> for HeadProbe {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let head = context.require_request()?;
        Ok(Self {
            summary: format!(
                "{} {} tenant={} id={} host={}",
                head.method,
                head.path(),
                head.header("x-tenant").unwrap_or("-"),
                head.path_param("id").unwrap_or("-"),
                head.host().unwrap_or("-"),
            ),
        })
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

impl ManagedDeps for HeadProbe {
    type Deps = TNil;
}

struct AllowAll;
impl SelfBuilt for AllowAll {}
impl<I: Identity> Guard<I> for AllowAll {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

struct PassThrough;
impl SelfBuilt for PassThrough {}
impl<R: Send> Interceptor<R> for PassThrough {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

/// Identity from `x-user`, so the `#[anonymous]` controller below has
/// something required to opt out of.
struct Subject(String);

impl Identity for Subject {
    fn sub(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Subject {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|sub| Subject(sub.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

/// Keeps `Option<Subject>` on a single `Via` path (see the note on
/// `controller::fixtures::SubjectViaOpt`).
struct SubjectViaOpt;

impl<S: Send + Sync> OptionalFromRequestPartsVia<S, SubjectViaOpt> for Subject {
    type Rejection = Response;

    async fn from_request_parts_via(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|sub| Subject(sub.to_owned())))
    }
}

#[controller]
struct PlainHeadController;

#[routes]
impl PlainHeadController {
    #[get("/head/plain/{id}")]
    async fn plain(&self, #[managed] probe: &mut HeadProbe) -> String {
        probe.summary.clone()
    }
}

#[controller]
struct GuardedHeadController;

#[routes]
impl GuardedHeadController {
    #[get("/head/guarded/{id}")]
    #[guard(AllowAll)]
    async fn guarded(&self, #[managed] probe: &mut HeadProbe) -> String {
        probe.summary.clone()
    }
}

/// Interceptors move managed acquisition inside the interceptor closure, where
/// the state is reached through a reference rather than the owned binding — a
/// separate codegen path for `.with_request(...)`.
#[controller]
struct InterceptedHeadController;

#[routes]
#[intercept(PassThrough)]
impl InterceptedHeadController {
    #[get("/head/intercepted/{id}")]
    async fn intercepted(&self, #[managed] probe: &mut HeadProbe) -> String {
        probe.summary.clone()
    }
}

/// `#[anonymous]` routes are emitted on the controller core with identity
/// extraction skipped — the head must still reach `acquire`.
#[controller]
struct AnonHeadController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl AnonHeadController {
    #[get("/head/anon/{id}")]
    #[anonymous]
    async fn anonymous(&self, #[managed] probe: &mut HeadProbe) -> String {
        probe.summary.clone()
    }

    #[get("/head/authed/{id}")]
    async fn authed(&self, #[managed] probe: &mut HeadProbe) -> String {
        format!("{}:{}", self.user.0, probe.summary)
    }
}

async fn head_router() -> r2e_core::http::Router {
    r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<PlainHeadController>()
        .register_controller::<GuardedHeadController>()
        .register_controller::<InterceptedHeadController>()
        .register_controller::<AnonHeadController>()
        .build()
}

#[r2e_core::test]
async fn request_head_reaches_acquire_in_every_handler_shape() {
    let router = head_router().await;

    for (path, label) in [
        ("/head/plain/42", "plain"),
        ("/head/guarded/42", "guarded"),
        ("/head/intercepted/42", "intercepted"),
        ("/head/anon/42", "anonymous"),
    ] {
        let (status, body) = send_get_with(
            router.clone(),
            path,
            &[("x-tenant", "acme"), ("host", "acme.example.test")],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label} route status");
        assert_eq!(
            body,
            format!("GET {path} tenant=acme id=42 host=acme.example.test"),
            "{label} route must see the request head at acquire time"
        );
    }
}

#[r2e_core::test]
async fn request_head_reaches_acquire_on_an_authenticated_route() {
    let router = head_router().await;

    let (status, body) = send_get_with(
        router.clone(),
        "/head/authed/7",
        &[
            ("x-user", "zoe"),
            ("x-tenant", "acme"),
            ("host", "acme.example.test"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        "zoe:GET /head/authed/7 tenant=acme id=7 host=acme.example.test"
    );

    // The identity route stays fail-closed; the anonymous sibling does not.
    let (status, _) = send_get_with(router.clone(), "/head/authed/7", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send_get_with(router, "/head/anon/7", &[]).await;
    assert_eq!(status, StatusCode::OK);
}

#[r2e_core::test]
async fn require_request_reports_a_missing_head_uniformly() {
    // A context built outside a request (a non-HTTP adapter, or a direct unit
    // test of a resource) carries no head.
    let state = ();
    let context = ManagedContext::new(&state, "AuditController", "record");
    let err = HeadProbe::acquire(context)
        .await
        .expect_err("no head means the probe cannot be acquired");

    let message = err.to_string();
    assert!(
        message.contains("AuditController::record"),
        "message must name controller::handler: {message}"
    );
    assert!(
        message.contains("requires the request head"),
        "message must explain the requirement: {message}"
    );

    let resp: Response = err.into();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── Guard arm/disarm ─────────────────────────────────────────────────────
//
// Moved out of `src/web/managed.rs` (the crate's inline `#[cfg(test)] mod
// tests`) when the runtime facade migration removed the last `tokio::` name
// from the crate's sources — tests belong in `tests/` per CLAUDE.md anyway.

struct Tracked {
    aborted: Arc<AtomicUsize>,
}

impl ManagedResource<Arc<AtomicUsize>> for Tracked {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, Arc<AtomicUsize>>) -> Result<Self, Self::Error> {
        Ok(Self {
            aborted: context.state.clone(),
        })
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {
        self.aborted.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn outcome_uses_http_status_class() {
    assert!(ManagedOutcome::from_status(StatusCode::CREATED).is_success());
    assert!(ManagedOutcome::from_status(StatusCode::TEMPORARY_REDIRECT).is_success());
    assert!(!ManagedOutcome::from_status(StatusCode::BAD_REQUEST).is_success());
    assert!(!ManagedOutcome::from_status(StatusCode::INTERNAL_SERVER_ERROR).is_success());
}

#[r2e_core::test]
async fn armed_guard_aborts_on_drop() {
    let aborted = Arc::new(AtomicUsize::new(0));
    let context = ManagedContext::new(&aborted, "Controller", "handler");
    let guard = ManagedGuard::<Tracked, _>::acquire(context).await.unwrap();
    drop(guard);
    assert_eq!(aborted.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn finalized_guard_is_disarmed() {
    let aborted = Arc::new(AtomicUsize::new(0));
    let context = ManagedContext::new(&aborted, "Controller", "handler");
    let mut guard = ManagedGuard::<Tracked, _>::acquire(context).await.unwrap();
    guard
        .finalize(&ManagedOutcome::from_status(StatusCode::OK))
        .await
        .unwrap();
    drop(guard);
    assert_eq!(aborted.load(Ordering::SeqCst), 0);
}
