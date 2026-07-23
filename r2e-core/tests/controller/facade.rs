//! Request-façade coverage for the Phase 4 controller refactor.
//!
//! Every controller with request-scoped fields (`#[inject(identity)]` and/or
//! `#[inject(request)]`) has its physical core built **once** (router-build
//! time). Per request, only the request-scoped values are extracted into a stack
//! façade (`__R2eRequest_<Name>`) that owns an `Arc` to the core; route methods
//! run on that façade. These tests prove identity isolation across concurrent
//! requests, the generic `#[inject(request)]` scope, guard/interceptor/managed
//! behavior, pre-auth ordering, SSE/WS identity, `Deref` access to core fields,
//! and that no `Arc<Controller>` is ever stashed in request extensions.

use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{
    Guard, GuardContext, Identity, Interceptor, InterceptorContext, ManagedErr, ManagedResource,
    PreAuthGuard, PreAuthGuardContext,
};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::fixtures::{req, FlaggingId, Subject, TenantId};

// ── 1. Concurrent identity isolation ───────────────────────────────────────

#[controller]
struct ConcurrentController {
    // Injected core field, reached through the façade `Deref` from the route.
    #[inject]
    barrier: Arc<tokio::sync::Barrier>,
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl ConcurrentController {
    #[get("/who")]
    async fn who(&self) -> String {
        // Rendezvous so every concurrent request is in-flight simultaneously,
        // then return *this request's* identity. If identity leaked across
        // requests, the returned subjects would not match the inputs.
        self.barrier.wait().await;
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn concurrent_identities_are_isolated() {
    const N: usize = 8;
    let barrier = Arc::new(tokio::sync::Barrier::new(N));
    let router = r2e_core::AppBuilder::new()
        .provide(barrier)
        .build_state()
        .await
        .register_controller::<ConcurrentController>()
        .build();

    let mut handles = Vec::new();
    for i in 0..N {
        let router = router.clone();
        handles.push(tokio::spawn(async move {
            let user = format!("user-{i}");
            let (status, body) = req(router, "/who", Some(&user), None).await;
            assert_eq!(status, StatusCode::OK);
            (user, body)
        }));
    }

    for h in handles {
        let (sent, got) = h.await.unwrap();
        assert_eq!(sent, got, "each response must see its own identity");
    }
}

// ── 2. Generic #[inject(request)] scope, isolated per request ───────────────

#[controller]
struct TenantController {
    #[inject]
    barrier: Arc<tokio::sync::Barrier>,
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl TenantController {
    #[get("/tenant")]
    async fn tenant(&self) -> String {
        self.barrier.wait().await;
        self.tenant.0.clone()
    }
}

#[r2e_core::test]
async fn request_scope_field_is_isolated() {
    const N: usize = 8;
    let barrier = Arc::new(tokio::sync::Barrier::new(N));
    let router = r2e_core::AppBuilder::new()
        .provide(barrier)
        .build_state()
        .await
        .register_controller::<TenantController>()
        .build();

    let mut handles = Vec::new();
    for i in 0..N {
        let router = router.clone();
        handles.push(tokio::spawn(async move {
            let tenant = format!("tenant-{i}");
            let (status, body) = req(router, "/tenant", None, Some(&tenant)).await;
            assert_eq!(status, StatusCode::OK);
            (tenant, body)
        }));
    }
    for h in handles {
        let (sent, got) = h.await.unwrap();
        assert_eq!(
            sent, got,
            "each response must see its own request-scoped value"
        );
    }
}

#[r2e_core::test]
async fn request_scope_field_rejection_propagates() {
    let barrier = Arc::new(tokio::sync::Barrier::new(1));
    let router = r2e_core::AppBuilder::new()
        .provide(barrier)
        .build_state()
        .await
        .register_controller::<TenantController>()
        .build();

    // Missing x-tenant header → the request-data extractor rejects with 400.
    let (status, _) = req(router, "/tenant", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── 3. Parameter identity keeps the core application-scoped ─────────────────

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
/// In the HList application-state model every provided bean — including the
/// controller's injected `dep` — is a member of the state, so routine
/// per-request state cloning also clones `dep`. This probe lives in the same
/// state and absorbs that identical per-request cloning, but is never pulled by
/// the core's `from_context`. Comparing the two counters therefore isolates
/// core (re)construction from routine state cloning.
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
struct ParamIdentityController {
    #[inject]
    #[allow(dead_code)]
    dep: CloneTracked,
}

#[routes]
impl ParamIdentityController {
    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: Subject) -> String {
        user.0
    }
}

#[r2e_core::test]
async fn parameter_identity_keeps_core_app_scoped() {
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
        .register_controller::<ParamIdentityController>()
        .build();

    // Baselines after build. `dep` is additionally cloned by the core's
    // `from_context`, so `core_clones` may exceed `state_clones` by a fixed
    // build-time amount — which we subtract out below.
    let core_base = core_clones.load(Ordering::SeqCst);
    let state_base = state_clones.load(Ordering::SeqCst);
    for i in 0..5 {
        let user = format!("p{i}");
        let (status, body) = req(router.clone(), "/me", Some(&user), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, user);
    }
    // Both counters absorb identical per-request state cloning, so their
    // per-request growth must match exactly. Divergence would mean the core was
    // rebuilt per request — an extra `from_context` clone of `dep` that the
    // state-only probe never sees.
    let core_delta = core_clones.load(Ordering::SeqCst) - core_base;
    let state_delta = state_clones.load(Ordering::SeqCst) - state_base;
    assert_eq!(
        core_delta, state_delta,
        "param-level identity must not make the core request-scoped \
         (core dep clones grew by {core_delta}, state-only probe by {state_delta})"
    );
}

// ── 4. Optional struct identity: authenticated + anonymous ──────────────────

#[controller]
struct OptionalController {
    #[inject(identity)]
    user: Option<Subject>,
}

#[routes]
impl OptionalController {
    #[get("/whoami")]
    async fn whoami(&self) -> String {
        match &self.user {
            Some(u) => u.0.clone(),
            None => "anonymous".to_string(),
        }
    }
}

#[r2e_core::test]
async fn optional_struct_identity_auth_and_anon() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<OptionalController>()
        .build();

    let (s1, b1) = req(router.clone(), "/whoami", Some("dave"), None).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(b1, "dave");

    let (s2, b2) = req(router, "/whoami", None, None).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b2, "anonymous");
}

// ── 5. Guard sees the same identity as the method ───────────────────────────

/// Spec named by the attribute; its bean dep is pulled once at wiring time
/// into the product guard (the graph-resolved decorator path).
struct RecordingGuard;

struct RecordingGuardReady {
    saw: Arc<Mutex<Vec<String>>>,
}

impl r2e_core::DecoratorSpec for RecordingGuard {
    type Product = RecordingGuardReady;
    type Deps = r2e_core::type_list::TCons<Arc<Mutex<Vec<String>>>, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> RecordingGuardReady {
        RecordingGuardReady { saw: ctx.get() }
    }
}

impl Guard<Subject> for RecordingGuardReady {
    fn check(
        &self,
        ctx: &GuardContext<'_, Subject>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let sub = ctx.identity.map(|i| i.sub().to_string());
        async move {
            if let Some(s) = sub {
                self.saw.lock().unwrap().push(s);
            }
            Ok(())
        }
    }
}

#[controller]
struct GuardedIdentityController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl GuardedIdentityController {
    #[get("/guarded")]
    #[guard(RecordingGuard)]
    async fn guarded(&self) -> String {
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn guard_sees_same_identity_as_method() {
    let saw: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let router = r2e_core::AppBuilder::new()
        .provide(saw.clone())
        .build_state()
        .await
        .register_controller::<GuardedIdentityController>()
        .build();

    for name in ["alice", "bob"] {
        let (status, body) = req(router.clone(), "/guarded", Some(name), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, name);
    }
    assert_eq!(*saw.lock().unwrap(), vec!["alice", "bob"]);
}

// ── 6. Pre-auth runs before identity extraction ─────────────────────────────

/// `allow` flag as a distinct bean type (the state has two `Arc<AtomicBool>`
/// values, which cannot coexist by type — newtype one of them).
#[derive(Clone)]
struct Allow(Arc<AtomicBool>);

struct GatePre;

struct GatePreReady {
    allow: Allow,
}

impl r2e_core::DecoratorSpec for GatePre {
    type Product = GatePreReady;
    type Deps = r2e_core::type_list::TCons<Allow, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> GatePreReady {
        GatePreReady { allow: ctx.get() }
    }
}

impl PreAuthGuard for GatePreReady {
    fn check(
        &self,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let allow = self.allow.0.load(Ordering::SeqCst);
        async move {
            if allow {
                Ok(())
            } else {
                Err(StatusCode::FORBIDDEN.into_response())
            }
        }
    }
}

#[controller]
struct PreAuthController {
    #[inject(identity)]
    user: FlaggingId,
}

#[routes]
impl PreAuthController {
    #[get("/gated")]
    #[pre_guard(GatePre)]
    async fn gated(&self) -> String {
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn pre_auth_runs_before_identity_extraction() {
    // Denied: pre-auth fires first, identity extraction must never run.
    let identity_ran = Arc::new(AtomicBool::new(false));
    let allow = Allow(Arc::new(AtomicBool::new(false)));
    let router = r2e_core::AppBuilder::new()
        .provide(identity_ran.clone())
        .provide(allow)
        .build_state()
        .await
        .register_controller::<PreAuthController>()
        .build();

    let (status, _) = req(router, "/gated", Some("eve"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        !identity_ran.load(Ordering::SeqCst),
        "pre-auth must reject before identity extraction"
    );

    // Allowed: pre-auth passes, identity extraction then runs.
    let identity_ran = Arc::new(AtomicBool::new(false));
    let allow = Allow(Arc::new(AtomicBool::new(true)));
    let router = r2e_core::AppBuilder::new()
        .provide(identity_ran.clone())
        .provide(allow)
        .build_state()
        .await
        .register_controller::<PreAuthController>()
        .build();

    let (status, body) = req(router, "/gated", Some("frank"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "frank");
    assert!(identity_ran.load(Ordering::SeqCst));
}

// ── 7. Interceptor runs once, before and after ──────────────────────────────

/// `after` counter as a distinct bean type (the state has two `Arc<AtomicUsize>`
/// values, which cannot coexist by type — newtype one of them).
#[derive(Clone)]
struct After(Arc<AtomicUsize>);

struct Counting;

struct CountingReady {
    before: Arc<AtomicUsize>,
    after: After,
}

impl r2e_core::DecoratorSpec for Counting {
    type Product = CountingReady;
    type Deps = r2e_core::type_list::TCons<
        Arc<AtomicUsize>,
        r2e_core::type_list::TCons<After, r2e_core::type_list::TNil>,
    >;

    fn build(self, ctx: &r2e_core::BeanContext) -> CountingReady {
        CountingReady {
            before: ctx.get(),
            after: ctx.get(),
        }
    }
}

impl<R: Send> Interceptor<R> for CountingReady {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move {
            self.before.fetch_add(1, Ordering::SeqCst);
            let r = next().await;
            self.after.0.fetch_add(1, Ordering::SeqCst);
            r
        }
    }
}

#[controller]
struct InterceptedIdentityController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl InterceptedIdentityController {
    #[get("/ix")]
    #[intercept(Counting)]
    async fn ix(&self) -> String {
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn interceptor_runs_once_around() {
    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let router = r2e_core::AppBuilder::new()
        .provide(before.clone())
        .provide(After(after.clone()))
        .build_state()
        .await
        .register_controller::<InterceptedIdentityController>()
        .build();

    for _ in 0..3 {
        let (status, body) = req(router.clone(), "/ix", Some("grace"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "grace");
    }
    assert_eq!(before.load(Ordering::SeqCst), 3);
    assert_eq!(after.load(Ordering::SeqCst), 3);
}

// ── 8. Managed resource commit / rollback on the façade ─────────────────────

struct Txn {
    released: Arc<Mutex<Vec<bool>>>,
}

impl<S: r2e_core::type_list::BeanLookup + Send + Sync> ManagedResource<S> for Txn {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(context: r2e_core::ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Ok(Txn {
            released: context
                .state
                .bean::<Arc<Mutex<Vec<bool>>>>()
                .expect("released handle must be provided"),
        })
    }

    async fn finalize(&mut self, outcome: &r2e_core::ManagedOutcome) -> Result<(), Self::Error> {
        self.released.lock().unwrap().push(outcome.is_success());
        Ok(())
    }

    fn abort(&mut self) {}
}

#[controller]
struct ManagedIdentityController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl ManagedIdentityController {
    #[get("/commit")]
    async fn commit(&self, #[managed] _tx: &mut Txn) -> Result<String, r2e_core::HttpError> {
        Ok(self.user.0.clone())
    }

    #[get("/rollback")]
    async fn rollback(&self, #[managed] _tx: &mut Txn) -> Result<String, r2e_core::HttpError> {
        let _ = &self.user;
        Err(r2e_core::HttpError::Internal("boom".into()))
    }
}

#[r2e_core::test]
async fn managed_resource_commit_and_rollback() {
    let released: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let router = r2e_core::AppBuilder::new()
        .provide(released.clone())
        .build_state()
        .await
        .register_controller::<ManagedIdentityController>()
        .build();

    let (s1, b1) = req(router.clone(), "/commit", Some("heidi"), None).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(b1, "heidi");

    let (s2, _) = req(router, "/rollback", Some("heidi"), None).await;
    assert_eq!(s2, StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(
        *released.lock().unwrap(),
        vec![true, false],
        "commit releases success=true, rollback releases success=false"
    );
}

struct FirstManaged {
    events: Arc<Mutex<Vec<&'static str>>>,
}

struct FailingFinalize {
    events: Arc<Mutex<Vec<&'static str>>>,
}

struct FailingAcquire;

impl<S: r2e_core::type_list::BeanLookup + Send + Sync> ManagedResource<S> for FirstManaged {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(context: r2e_core::ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Ok(Self {
            events: context.state.bean().expect("events bean must be provided"),
        })
    }

    async fn finalize(&mut self, _outcome: &r2e_core::ManagedOutcome) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push("first_finalize");
        Ok(())
    }

    fn abort(&mut self) {
        self.events.lock().unwrap().push("first_abort");
    }
}

impl<S: r2e_core::type_list::BeanLookup + Send + Sync> ManagedResource<S> for FailingFinalize {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(context: r2e_core::ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Ok(Self {
            events: context.state.bean().expect("events bean must be provided"),
        })
    }

    async fn finalize(&mut self, _outcome: &r2e_core::ManagedOutcome) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push("second_finalize");
        Err(ManagedErr(r2e_core::HttpError::internal("finalize failed")))
    }

    fn abort(&mut self) {
        self.events.lock().unwrap().push("second_abort");
    }
}

impl<S: Send + Sync> ManagedResource<S> for FailingAcquire {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(_context: r2e_core::ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Err(ManagedErr(r2e_core::HttpError::internal("acquire failed")))
    }

    async fn finalize(&mut self, _outcome: &r2e_core::ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

#[controller]
struct ManagedSafetyController;

#[routes]
impl ManagedSafetyController {
    #[get("/managed/finalize-all")]
    async fn finalize_all(
        &self,
        #[managed] _first: &mut FirstManaged,
        #[managed] _second: &mut FailingFinalize,
    ) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[get("/managed/partial-acquire")]
    async fn partial_acquire(
        &self,
        #[managed] _first: &mut FirstManaged,
        #[managed] _second: &mut FailingAcquire,
    ) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[r2e_core::test]
async fn managed_resources_finalize_all_and_abort_partial_acquisition() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let router = r2e_core::AppBuilder::new()
        .provide(events.clone())
        .build_state()
        .await
        .register_controller::<ManagedSafetyController>()
        .build();

    let (status, _) = req(router.clone(), "/managed/finalize-all", None, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["second_finalize", "first_finalize", "second_abort"]
    );

    events.lock().unwrap().clear();
    let (status, _) = req(router, "/managed/partial-acquire", None, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(*events.lock().unwrap(), vec!["first_abort"]);
}

// ── 9. SSE identity ─────────────────────────────────────────────────────────

#[controller]
struct SseIdentityController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl SseIdentityController {
    #[sse("/sse")]
    async fn sse(
        &self,
    ) -> impl futures_core::Stream<
        Item = Result<r2e_core::http::response::SseEvent, std::convert::Infallible>,
    > {
        let sub = self.user.0.clone();
        use tokio_stream::wrappers::ReceiverStream;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Ok(r2e_core::http::response::SseEvent::default().data(sub)))
            .await
            .unwrap();
        drop(tx);
        ReceiverStream::new(rx)
    }
}

#[r2e_core::test]
async fn sse_identity_is_correct() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<SseIdentityController>()
        .build();

    let (status, body) = req(router.clone(), "/sse", Some("ivan"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("ivan"),
        "SSE body should carry identity: {body:?}"
    );

    // No identity header → request-data extraction rejects before the stream.
    let (status, _) = req(router, "/sse", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── 10. WebSocket identity (upgrade path) ───────────────────────────────────
//
// Gated behind the `ws` feature (r2e-core's WebSocket support). Run with
// `cargo test -p r2e-core --test controller_facade --features ws`.

#[cfg(feature = "ws")]
#[controller]
struct WsIdentityController {
    #[inject(identity)]
    user: Subject,
}

#[cfg(feature = "ws")]
#[routes]
impl WsIdentityController {
    #[ws("/ws")]
    async fn ws(&self, mut ws: r2e_core::ws::WsStream) {
        // Identity is owned by the façade for the whole socket lifetime.
        let sub = self.user.0.clone();
        ws.send_text(&sub).await.ok();
    }
}

#[cfg(feature = "ws")]
async fn ws_upgrade(router: r2e_core::http::Router, user: Option<&str>) -> StatusCode {
    use tower::ServiceExt;

    let mut b = Request::builder()
        .uri("/ws")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13");
    if let Some(u) = user {
        b = b.header("x-user", u);
    }
    router
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[cfg(feature = "ws")]
#[r2e_core::test]
async fn ws_identity_extracted_on_upgrade() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<WsIdentityController>()
        .build();

    // The request-data extractor (identity) runs before the `WebSocketUpgrade`
    // extractor on the façade WS path.
    //
    // Without identity → 401: identity extraction rejects first; the upgrade is
    // never attempted.
    assert_eq!(
        ws_upgrade(router.clone(), None).await,
        StatusCode::UNAUTHORIZED
    );
    // With identity → identity extraction succeeds and we reach the upgrade
    // machinery. A `tower::oneshot` call cannot complete a real protocol upgrade
    // (there is no live connection), so axum responds 426 UPGRADE_REQUIRED — but
    // crucially NOT 401, proving identity was already bound on the WS path.
    assert_eq!(
        ws_upgrade(router, Some("judy")).await,
        StatusCode::UPGRADE_REQUIRED
    );
}

// ── 11. Core injected fields reachable via Deref ────────────────────────────

#[controller]
struct DerefController {
    #[inject]
    label: String,
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl DerefController {
    #[get("/deref")]
    async fn deref(&self) -> String {
        // `self.label` is a core field reached through the façade `Deref`;
        // `self.user` is a façade field.
        format!("{}:{}", self.label, self.user.0)
    }
}

#[r2e_core::test]
async fn core_fields_reachable_via_deref() {
    let router = r2e_core::AppBuilder::new()
        .provide("core".to_string())
        .build_state()
        .await
        .register_controller::<DerefController>()
        .build();

    let (status, body) = req(router, "/deref", Some("ken"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "core:ken");
}

// ── 12. No Arc<Controller> request extension is installed ────────────────────

#[controller]
struct NoExtController {
    #[inject(identity)]
    user: Subject,
}

async fn assert_no_controller_extension(
    request: Request,
    next: r2e_core::http::middleware::Next,
) -> Response {
    assert!(
        request.extensions().get::<Arc<NoExtController>>().is_none(),
        "the façade path must not place the controller core in request extensions"
    );
    next.run(request).await
}

#[routes]
impl NoExtController {
    #[get("/noext")]
    #[middleware(assert_no_controller_extension)]
    async fn noext(&self) -> String {
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn no_controller_arc_request_extension() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<NoExtController>()
        .build();

    let (status, body) = req(router, "/noext", Some("laura"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "laura");
}
