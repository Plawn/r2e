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

use http_body_util::BodyExt;
use r2e_core::http::extract::{FromRequestParts, OptionalFromRequestParts};
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{
    Guard, GuardContext, Identity, Interceptor, InterceptorContext, ManagedErr, ManagedResource,
    PreAuthGuard, PreAuthGuardContext,
};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

// ── Shared request-scoped types ────────────────────────────────────────────

/// An identity extracted from the `x-user` header. Implements `Identity` so it
/// can drive guards and `Option<Subject>` (struct-level optional identity).
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
            .and_then(|v| v.to_str().ok())
            .map(|s| Subject(s.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for Subject {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| Subject(s.to_owned())))
    }
}

/// A non-identity request-scoped value from the `x-tenant` header, proving the
/// generic `#[inject(request)]` scope works for any `FromRequestParts` type.
struct TenantId(String);

impl<S: Send + Sync> FromRequestParts<S> for TenantId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-tenant")
            .and_then(|v| v.to_str().ok())
            .map(|s| TenantId(s.to_owned()))
            .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// GET with optional `x-user` / `x-tenant` headers.
async fn req(
    router: r2e_core::http::Router,
    path: &str,
    user: Option<&str>,
    tenant: Option<&str>,
) -> (StatusCode, String) {
    let mut b = Request::builder().uri(path);
    if let Some(u) = user {
        b = b.header("x-user", u);
    }
    if let Some(t) = tenant {
        b = b.header("x-tenant", t);
    }
    let resp = router
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_string(resp).await)
}

// ── 1. Concurrent identity isolation ───────────────────────────────────────

#[derive(Clone)]
struct ConcurrentState {
    barrier: Arc<tokio::sync::Barrier>,
}

#[controller(state = ConcurrentState)]
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
    let state = ConcurrentState {
        barrier: Arc::new(tokio::sync::Barrier::new(N)),
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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

#[derive(Clone)]
struct TenantState {
    barrier: Arc<tokio::sync::Barrier>,
}

#[controller(state = TenantState)]
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
    let state = TenantState {
        barrier: Arc::new(tokio::sync::Barrier::new(N)),
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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
    let state = TenantState {
        barrier: Arc::new(tokio::sync::Barrier::new(1)),
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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

struct ParamState {
    dep: CloneTracked,
}

impl Clone for ParamState {
    fn clone(&self) -> Self {
        Self {
            dep: CloneTracked {
                clones: Arc::clone(&self.dep.clones),
            },
        }
    }
}

#[controller(state = ParamState)]
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
    let clones = Arc::new(AtomicUsize::new(0));
    let state = ParamState {
        dep: CloneTracked {
            clones: Arc::clone(&clones),
        },
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
        .register_controller::<ParamIdentityController>()
        .build();

    let after_build = clones.load(Ordering::SeqCst);
    for i in 0..5 {
        let user = format!("p{i}");
        let (status, body) = req(router.clone(), "/me", Some(&user), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, user);
    }
    assert_eq!(
        clones.load(Ordering::SeqCst),
        after_build,
        "param-level identity must not make the core request-scoped"
    );
}

// ── 4. Optional struct identity: authenticated + anonymous ──────────────────

#[derive(Clone)]
struct OptState;

#[controller(state = OptState)]
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
        .with_state(OptState)
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

#[derive(Clone)]
struct GuardState {
    saw: Arc<Mutex<Vec<String>>>,
}

struct RecordingGuard;

impl Guard<GuardState, Subject> for RecordingGuard {
    fn check(
        &self,
        state: &GuardState,
        ctx: &GuardContext<'_, Subject>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let saw = state.saw.clone();
        let sub = ctx.identity.map(|i| i.sub().to_string());
        async move {
            if let Some(s) = sub {
                saw.lock().unwrap().push(s);
            }
            Ok(())
        }
    }
}

#[controller(state = GuardState)]
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
    let saw = Arc::new(Mutex::new(Vec::new()));
    let state = GuardState { saw: saw.clone() };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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

#[derive(Clone)]
struct PreAuthState {
    identity_ran: Arc<AtomicBool>,
    allow: Arc<AtomicBool>,
}

/// Identity that records whether it was ever extracted.
struct FlaggingId(String);

impl FromRequestParts<PreAuthState> for FlaggingId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        state: &PreAuthState,
    ) -> Result<Self, Self::Rejection> {
        state.identity_ran.store(true, Ordering::SeqCst);
        parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| FlaggingId(s.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

struct GatePre;

impl PreAuthGuard<PreAuthState> for GatePre {
    fn check(
        &self,
        state: &PreAuthState,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let allow = state.allow.load(Ordering::SeqCst);
        async move {
            if allow {
                Ok(())
            } else {
                Err(StatusCode::FORBIDDEN.into_response())
            }
        }
    }
}

#[controller(state = PreAuthState)]
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
    let denied_state = PreAuthState {
        identity_ran: Arc::new(AtomicBool::new(false)),
        allow: Arc::new(AtomicBool::new(false)),
    };
    let identity_ran = denied_state.identity_ran.clone();
    let router = r2e_core::AppBuilder::new()
        .with_state(denied_state)
        .register_controller::<PreAuthController>()
        .build();

    let (status, _) = req(router, "/gated", Some("eve"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        !identity_ran.load(Ordering::SeqCst),
        "pre-auth must reject before identity extraction"
    );

    // Allowed: pre-auth passes, identity extraction then runs.
    let ok_state = PreAuthState {
        identity_ran: Arc::new(AtomicBool::new(false)),
        allow: Arc::new(AtomicBool::new(true)),
    };
    let identity_ran = ok_state.identity_ran.clone();
    let router = r2e_core::AppBuilder::new()
        .with_state(ok_state)
        .register_controller::<PreAuthController>()
        .build();

    let (status, body) = req(router, "/gated", Some("frank"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "frank");
    assert!(identity_ran.load(Ordering::SeqCst));
}

// ── 7. Interceptor runs once, before and after ──────────────────────────────

#[derive(Clone)]
struct IxState {
    before: Arc<AtomicUsize>,
    after: Arc<AtomicUsize>,
}

struct Counting;

impl<R: Send> Interceptor<R, IxState> for Counting {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext<'_, IxState>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let before = ctx.state.before.clone();
        let after = ctx.state.after.clone();
        async move {
            before.fetch_add(1, Ordering::SeqCst);
            let r = next().await;
            after.fetch_add(1, Ordering::SeqCst);
            r
        }
    }
}

#[controller(state = IxState)]
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
    let state = IxState {
        before: Arc::new(AtomicUsize::new(0)),
        after: Arc::new(AtomicUsize::new(0)),
    };
    let before = state.before.clone();
    let after = state.after.clone();
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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

#[derive(Clone)]
struct ManagedState {
    released: Arc<Mutex<Vec<bool>>>,
}

struct Txn {
    released: Arc<Mutex<Vec<bool>>>,
}

impl ManagedResource<ManagedState> for Txn {
    type Error = ManagedErr<r2e_core::HttpError>;

    async fn acquire(state: &ManagedState) -> Result<Self, Self::Error> {
        Ok(Txn {
            released: state.released.clone(),
        })
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        self.released.lock().unwrap().push(success);
        Ok(())
    }
}

#[controller(state = ManagedState)]
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
    let released = Arc::new(Mutex::new(Vec::new()));
    let state = ManagedState {
        released: released.clone(),
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
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

// ── 9. SSE identity ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct SseState;

#[controller(state = SseState)]
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
        .with_state(SseState)
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
#[derive(Clone)]
struct WsState;

#[cfg(feature = "ws")]
#[controller(state = WsState)]
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
        .with_state(WsState)
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

#[derive(Clone)]
struct DerefState {
    label: String,
}

#[controller(state = DerefState)]
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
    let state = DerefState {
        label: "core".to_string(),
    };
    let router = r2e_core::AppBuilder::new()
        .with_state(state)
        .register_controller::<DerefController>()
        .build();

    let (status, body) = req(router, "/deref", Some("ken"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "core:ken");
}

// ── 12. No Arc<Controller> request extension is installed ────────────────────

#[derive(Clone)]
struct NoExtState;

#[controller(state = NoExtState)]
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
        .with_state(NoExtState)
        .register_controller::<NoExtController>()
        .build();

    let (status, body) = req(router, "/noext", Some("laura"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "laura");
}
