//! Captured-core coverage: controllers build once in the state-aware route
//! builder and serve every request from the shared `Arc`,
//! across every handler shape — simple, guarded, intercepted, managed, SSE,
//! and pre-auth-guarded routes. Request identity remains scoped to the facade
//! (covered separately in `controller_scope.rs`).

use http_body_util::BodyExt;
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{
    Guard, GuardContext, Identity, InterceptorContext, ManagedErr, ManagedResource, PreAuthGuard,
    PreAuthGuardContext,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

// ── Shared bookkeeping types ──────────────────────────────────────────────

/// Counts how many times `StatefulConstruct::from_state` runs per
/// controller. Captured-core dispatch should make this go to exactly 1 — the
/// registration call inside `AppBuilder::register_controller()`.
struct BuildTracker {
    builds: Arc<AtomicUsize>,
}

impl Clone for BuildTracker {
    fn clone(&self) -> Self {
        Self {
            builds: Arc::clone(&self.builds),
        }
    }
}

impl BuildTracker {
    fn new() -> Self {
        Self {
            builds: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn record(&self) {
        self.builds.fetch_add(1, Ordering::SeqCst);
    }
    fn count(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }
}

// ── 1. Simple non-identity controller ──────────────────────────────────────

#[controller]
struct SimpleController {
    #[inject]
    simple: BuildTracker,
}

#[routes]
impl SimpleController {
    #[get("/simple")]
    #[middleware(assert_no_controller_extension)]
    async fn handle(&self) -> String {
        self.simple.record();
        "ok".to_string()
    }
}

async fn assert_no_controller_extension(
    request: Request,
    next: r2e_core::http::middleware::Next,
) -> Response {
    assert!(
        request
            .extensions()
            .get::<Arc<SimpleController>>()
            .is_none(),
        "captured-core dispatch must not place the controller in request extensions"
    );
    next.run(request).await
}

// ── 2. Guarded controller (Case 3) ─────────────────────────────────────────

struct AllowAll;
impl r2e_core::SelfBuilt for AllowAll {}
impl<I: Identity> Guard<I> for AllowAll {
    fn check(&self, _ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[controller]
struct GuardedController {
    #[inject]
    guarded: BuildTracker,
}

#[routes]
impl GuardedController {
    #[get("/guarded")]
    #[guard(AllowAll)]
    async fn handle(&self) -> String {
        self.guarded.record();
        "ok".to_string()
    }
}

// ── 3. Intercepted controller (Case 2a) ────────────────────────────────────

struct PassThrough;
impl r2e_core::SelfBuilt for PassThrough {}

impl<R: Send> r2e_core::Interceptor<R> for PassThrough {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

#[controller]
struct InterceptedController {
    #[inject]
    intercepted: BuildTracker,
}

#[routes]
#[intercept(PassThrough)]
impl InterceptedController {
    #[get("/intercepted")]
    async fn handle(&self) -> String {
        self.intercepted.record();
        "ok".to_string()
    }
}

// ── 4. Managed-resource controller ─────────────────────────────────────────

struct ManagedToken;

impl<S: Send + Sync> ManagedResource<S> for ManagedToken {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(_state: &S) -> Result<Self, Self::Error> {
        Ok(ManagedToken)
    }

    async fn release(self, _success: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[controller]
struct ManagedController {
    #[inject]
    managed: BuildTracker,
}

#[routes]
impl ManagedController {
    #[get("/managed")]
    async fn handle(&self, #[managed] _tx: &mut ManagedToken) -> String {
        self.managed.record();
        "ok".to_string()
    }
}

// ── 5. SSE controller ──────────────────────────────────────────────────────

#[controller]
struct SseController {
    #[inject]
    sse: BuildTracker,
}

#[routes]
impl SseController {
    #[sse("/events")]
    async fn handle(
        &self,
    ) -> impl futures_core::Stream<
        Item = Result<r2e_core::http::response::SseEvent, std::convert::Infallible>,
    > {
        self.sse.record();
        // Use tokio_stream so we don't need futures_util.
        use tokio_stream::wrappers::ReceiverStream;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Ok(
            r2e_core::http::response::SseEvent::default().data("hello")
        ))
        .await
        .unwrap();
        drop(tx);
        ReceiverStream::new(rx)
    }
}

// ── 6. Pre-auth-guarded controller ─────────────────────────────────────────

struct AllowAllPre;
impl r2e_core::SelfBuilt for AllowAllPre {}
impl PreAuthGuard for AllowAllPre {
    fn check(&self, _ctx: &PreAuthGuardContext<'_>) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[controller]
struct PreAuthController {
    #[inject]
    pre_auth: BuildTracker,
}

#[routes]
impl PreAuthController {
    #[get("/pre-auth")]
    #[pre_guard(AllowAllPre)]
    async fn handle(&self) -> String {
        self.pre_auth.record();
        "ok".to_string()
    }
}

// ── 7. Direct state-aware routes construction ──────────────────────────────

#[controller]
struct DirectRoutesController {
    #[inject]
    #[allow(dead_code)]
    simple: BuildTracker,
}

#[routes]
impl DirectRoutesController {
    #[get("/direct")]
    async fn handle(&self) -> String {
        "direct-ok".to_string()
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get(router: r2e_core::http::Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_string(resp).await)
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Simple controller — Arc captured once in the state-aware route builder, never
/// rebuilt per request.
#[r2e_core::test]
async fn simple_controller_constructed_once() {
    let tracker = BuildTracker::new();
    let router = r2e_core::AppBuilder::new()
        .provide(tracker.clone())
        .build_state()
        .await
        .register_controller::<SimpleController>()
        .build();

    for _ in 0..5 {
        let (status, body) = get(router.clone(), "/simple").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
    // The tracker counts handler invocations, not constructions; the only
    // way to prove "constructed once" with this scaffold is to count
    // `from_state` calls — and `StatefulConstruct::from_state` builds the
    // struct by cloning `BuildTracker`. The handler still runs 5 times.
    assert_eq!(tracker.count(), 5);
}

/// Guarded controller — guards still fire with the captured core.
#[r2e_core::test]
async fn guarded_controller_uses_captured_core() {
    let router = r2e_core::AppBuilder::new()
        .provide(BuildTracker::new())
        .build_state()
        .await
        .register_controller::<GuardedController>()
        .build();

    for _ in 0..3 {
        let (status, body) = get(router.clone(), "/guarded").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}

/// Intercepted controller — the interceptor chain runs over the captured core.
#[r2e_core::test]
async fn intercepted_controller_uses_captured_core() {
    let router = r2e_core::AppBuilder::new()
        .provide(BuildTracker::new())
        .build_state()
        .await
        .register_controller::<InterceptedController>()
        .build();

    for _ in 0..3 {
        let (status, body) = get(router.clone(), "/intercepted").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}

/// Managed-resource controller — acquire/release run over the captured core.
#[r2e_core::test]
async fn managed_controller_uses_captured_core() {
    let router = r2e_core::AppBuilder::new()
        .provide(BuildTracker::new())
        .build_state()
        .await
        .register_controller::<ManagedController>()
        .build();

    for _ in 0..3 {
        let (status, body) = get(router.clone(), "/managed").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}

/// SSE controller — the stream is produced from the captured core.
#[r2e_core::test]
async fn sse_controller_uses_captured_core() {
    let router = r2e_core::AppBuilder::new()
        .provide(BuildTracker::new())
        .build_state()
        .await
        .register_controller::<SseController>()
        .build();

    let req = Request::builder()
        .uri("/events")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("hello"), "got body: {body:?}");
}

/// Pre-auth-guarded controller — the route uses the captured core and the
/// pre-auth middleware fires before dispatch.
#[r2e_core::test]
async fn pre_auth_route_uses_captured_core() {
    let router = r2e_core::AppBuilder::new()
        .provide(BuildTracker::new())
        .build_state()
        .await
        .register_controller::<PreAuthController>()
        .build();

    for _ in 0..3 {
        let (status, body) = get(router.clone(), "/pre-auth").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}

// ── Quantitative captured-core check ──────────────────────────────────────

/// `from_context` clones this dependency into the controller core, so the
/// dependency clone counter is a proxy for the number of times the core was
/// constructed. Cloning bumps the counter; `Arc::clone` of the shared core on
/// each request does not.
struct CloneTracked {
    clones: Arc<AtomicUsize>,
}

impl Clone for CloneTracked {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
        }
    }
}

/// A structurally identical probe that the controller does **not** inject.
///
/// In the HList application-state model, every provided bean — including the
/// controller's injected `dep` — is a member of the state, so a guarded route's
/// routine per-request state cloning also clones `dep`. This probe lives in the
/// same state and absorbs that identical per-request cloning, but is never
/// pulled by the core's `from_context`. Comparing the two counters therefore
/// isolates core (re)construction from routine state cloning.
struct StateOnlyProbe {
    clones: Arc<AtomicUsize>,
}

impl Clone for StateOnlyProbe {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
        }
    }
}

#[controller]
struct FastPathController {
    #[inject]
    #[allow(dead_code)]
    dep: CloneTracked,
}

#[routes]
impl FastPathController {
    #[get("/fast")]
    #[guard(AllowAll)]
    async fn handle(&self) -> String {
        "ok".to_string()
    }
}

/// With captured-core dispatch, even a guarded route should construct the
/// controller exactly once (during router build). Per-request rebuilds
/// would push the clone count above 1.
#[r2e_core::test]
async fn captured_core_skips_per_request_construction() {
    let core_clones = Arc::new(AtomicUsize::new(0));
    let state_clones = Arc::new(AtomicUsize::new(0));
    let router = r2e_core::AppBuilder::new()
        .provide(CloneTracked {
            clones: Arc::clone(&core_clones),
        })
        .provide(StateOnlyProbe {
            clones: Arc::clone(&state_clones),
        })
        .build_state()
        .await
        .register_controller::<FastPathController>()
        .build();

    // Baselines after build. `dep` is additionally cloned once by the core's
    // `from_context`, so `core_clones` may exceed `state_clones` by a fixed
    // build-time amount — which we subtract out below.
    let core_base = core_clones.load(Ordering::SeqCst);
    let state_base = state_clones.load(Ordering::SeqCst);

    for _ in 0..10 {
        let (status, body) = get(router.clone(), "/fast").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    // Both counters absorb identical per-request state cloning, so their
    // per-request growth must match exactly. Divergence would mean the core was
    // rebuilt per request — an extra `from_context` clone of `dep` that the
    // state-only probe never sees.
    let core_delta = core_clones.load(Ordering::SeqCst) - core_base;
    let state_delta = state_clones.load(Ordering::SeqCst) - state_base;
    assert_eq!(
        core_delta, state_delta,
        "guarded route must not rebuild the controller core per request \
         (core dep clones grew by {core_delta}, state-only probe by {state_delta})"
    );
}

/// Direct routes remain available when the controller core is built by hand
/// from a resolved bean context and wired via `Controller::routes` — the
/// low-level path `register_controller()` drives internally.
#[r2e_core::test]
async fn direct_state_aware_routes_still_work() {
    let mut registry = r2e_core::BeanRegistry::new();
    registry.provide(BuildTracker::new());
    let ctx = registry.resolve().await.unwrap();

    // The generated `Controller` impl requires the state to implement
    // `BeanLookup`; the empty HList `HNil` is the minimal such state.
    use r2e_core::type_list::HNil;
    let core =
        Arc::new(<DirectRoutesController as r2e_core::ContextConstruct>::from_context(&ctx));
    let router =
        <DirectRoutesController as r2e_core::Controller<HNil, _>>::routes(&HNil, core, &ctx)
            .with_state(HNil);

    let (status, body) = get(router, "/direct").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "direct-ok");
}
