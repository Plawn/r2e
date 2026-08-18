//! Controller-level decorators on the `#[routes]` impl block (`#[guard]`,
//! `#[pre_guard]`, `#[roles]` — task #906).
//!
//! Semantics under test:
//! - controller guards/pre-guards apply to every route and run BEFORE the
//!   method-level ones (cumulative, not replacing);
//! - the controller set is built ONCE and shared — one stateful bucket per
//!   controller, with `method_name: "*"` in its contexts;
//! - `#[anonymous]` opts out of the controller's post-auth guards but keeps
//!   its pre-guards (pre-auth checks don't depend on identity);
//! - SSE routes are covered by controller guards like plain routes.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::beans::BeanContext;
use r2e_core::guards::PreAuthGuardContext;
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Body, StatusCode};
use r2e_core::prelude::*;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{AppBuilder, DecoratorSpec, GuardContext, GuardError, Identity};

// ── Fixtures ────────────────────────────────────────────────────────────────

/// Shared journal: guard-check events (`label@method_name`) and spec builds
/// (one label per `DecoratorSpec::build`), so tests can assert order,
/// context bucket, and build-once.
#[derive(Clone, Default)]
struct CallLog {
    events: Arc<Mutex<Vec<String>>>,
    builds: Arc<Mutex<Vec<&'static str>>>,
}

impl CallLog {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
    fn builds_of(&self, label: &str) -> usize {
        self.builds.lock().unwrap().iter().filter(|l| **l == label).count()
    }
}

/// Identity extracted from the `x-user` header.
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

/// Post-auth guard spec: logs `label@method_name`, counts a shared-instance
/// hit, and rejects with 403 when the `x-deny` header names its label.
struct Recording {
    label: &'static str,
}

impl Recording {
    fn labeled(label: &'static str) -> Self {
        Self { label }
    }
}

struct RecordingGuard {
    log: CallLog,
    label: &'static str,
    hits: AtomicUsize,
}

impl DecoratorSpec for Recording {
    type Product = RecordingGuard;
    type Deps = TCons<CallLog, TNil>;

    fn build(self, ctx: &BeanContext) -> RecordingGuard {
        let log: CallLog = ctx.get();
        log.builds.lock().unwrap().push(self.label);
        RecordingGuard {
            log,
            label: self.label,
            hits: AtomicUsize::new(0),
        }
    }
}

impl<I: Identity> Guard<I> for RecordingGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let hit = self.hits.fetch_add(1, Ordering::SeqCst) + 1;
        self.log
            .events
            .lock()
            .unwrap()
            .push(format!("{}@{}#{}", self.label, ctx.method_name, hit));
        let denied = ctx
            .headers
            .get("x-deny")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == self.label);
        async move {
            if denied {
                Err(GuardError::forbidden("denied").into())
            } else {
                Ok(())
            }
        }
    }
}

/// Pre-auth guard spec: logs `pre-label@method_name`, rejects with 403 when
/// the `x-predeny` header names its label.
struct RecordingPre {
    label: &'static str,
}

impl RecordingPre {
    fn labeled(label: &'static str) -> Self {
        Self { label }
    }
}

struct RecordingPreGuard {
    log: CallLog,
    label: &'static str,
}

impl DecoratorSpec for RecordingPre {
    type Product = RecordingPreGuard;
    type Deps = TCons<CallLog, TNil>;

    fn build(self, ctx: &BeanContext) -> RecordingPreGuard {
        let log: CallLog = ctx.get();
        log.builds.lock().unwrap().push(self.label);
        RecordingPreGuard {
            log,
            label: self.label,
        }
    }
}

impl PreAuthGuard for RecordingPreGuard {
    fn check(
        &self,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        self.log
            .events
            .lock()
            .unwrap()
            .push(format!("pre-{}@{}", self.label, ctx.method_name));
        let denied = ctx
            .headers
            .get("x-predeny")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == self.label);
        async move {
            if denied {
                Err(GuardError::forbidden("pre-denied").into())
            } else {
                Ok(())
            }
        }
    }
}

// ── Controllers ─────────────────────────────────────────────────────────────

/// Controller-level + method-level guards and pre-guards, cumulated.
#[controller(path = "/order")]
struct OrderController {}

#[routes]
#[guard(Recording::labeled("ctrl"))]
#[pre_guard(RecordingPre::labeled("ctrlpre"))]
impl OrderController {
    #[get("/a")]
    #[guard(Recording::labeled("method"))]
    #[pre_guard(RecordingPre::labeled("methodpre"))]
    async fn a(&self) -> &'static str {
        "a"
    }

    #[get("/b")]
    async fn b(&self) -> &'static str {
        "b"
    }

    #[sse("/stream")]
    async fn stream(
        &self,
    ) -> impl futures_core::Stream<
        Item = Result<r2e_core::http::response::SseEvent, std::convert::Infallible>,
    > {
        use tokio_stream::wrappers::ReceiverStream;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Ok(r2e_core::http::response::SseEvent::default().data("s")))
            .await
            .unwrap();
        drop(tx);
        ReceiverStream::new(rx)
    }
}

/// Struct identity + `#[anonymous]`: the anonymous route skips the
/// controller's post-auth guard but keeps its pre-guard.
#[controller(path = "/anon")]
struct AnonController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
#[guard(Recording::labeled("cg"))]
#[pre_guard(RecordingPre::labeled("cp"))]
impl AnonController {
    #[get("/open")]
    #[anonymous]
    async fn open(&self) -> &'static str {
        "open"
    }

    #[get("/secure")]
    async fn secure(&self) -> String {
        self.user.0.clone()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn get(
    router: &r2e_core::http::Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    crate::support::send(router.clone(), "GET", path, headers, Body::empty()).await
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// AC1 + AC2 + AC3: controller guards/pre-guards apply to all routes, run
/// before the method-level ones, and cumulate with them. The controller
/// contexts carry `method_name: "*"`.
#[r2e_core::test]
async fn controller_checks_run_before_method_checks() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .provide(log.clone())
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    let (status, body) = get(&router, "/order/a", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "a");
    assert_eq!(
        log.events(),
        vec![
            "pre-ctrlpre@*",
            "pre-methodpre@a",
            "ctrl@*#1",
            "method@a#1",
        ],
        "controller pre-guard, then method pre-guard, then controller guard, then method guard"
    );

    // A route with no method-level decorators still gets the controller's.
    log.clear();
    let (status, body) = get(&router, "/order/b", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "b");
    assert_eq!(log.events(), vec!["pre-ctrlpre@*", "ctrl@*#2"]);
}

/// The shared-set property: the controller guard is built once per controller
/// registration and every route hits the SAME instance (the `#N` counter is
/// monotonic across routes — one stateful bucket for the whole controller).
#[r2e_core::test]
async fn controller_set_is_built_once_and_shared() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .provide(log.clone())
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    let _ = get(&router, "/order/a", &[]).await;
    let _ = get(&router, "/order/b", &[]).await;
    let _ = get(&router, "/order/a", &[]).await;

    assert_eq!(log.builds_of("ctrl"), 1, "one build for the whole controller");
    assert_eq!(log.builds_of("ctrlpre"), 1);
    // The hit counter lives on the instance: 3 requests → #1, #2, #3 across
    // DIFFERENT routes proves they share it.
    let ctrl_hits: Vec<String> = log
        .events()
        .into_iter()
        .filter(|e| e.starts_with("ctrl@"))
        .collect();
    assert_eq!(ctrl_hits, vec!["ctrl@*#1", "ctrl@*#2", "ctrl@*#3"]);
}

/// Cumulative rejection: the controller guard can reject any route; a method
/// guard only its own.
#[r2e_core::test]
async fn controller_and_method_guards_cumulate() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .provide(log.clone())
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    // Controller guard rejects everything…
    let (status, _) = get(&router, "/order/a", &[("x-deny", "ctrl")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/b", &[("x-deny", "ctrl")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // …and it short-circuits BEFORE the method guard runs.
    let last = log.events().last().cloned().unwrap();
    assert!(
        last.starts_with("ctrl@"),
        "method guard must not run after a controller rejection: {last}"
    );

    // A method guard only affects its own route.
    let (status, _) = get(&router, "/order/a", &[("x-deny", "method")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/b", &[("x-deny", "method")]).await;
    assert_eq!(status, StatusCode::OK);

    // Same for pre-guards.
    let (status, _) = get(&router, "/order/b", &[("x-predeny", "ctrlpre")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/a", &[("x-predeny", "methodpre")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/b", &[("x-predeny", "methodpre")]).await;
    assert_eq!(status, StatusCode::OK);
}

/// SSE routes are covered by the controller guard and pre-guard like plain
/// routes (their codegen path differs — no interceptor chain).
#[r2e_core::test]
async fn controller_guards_cover_sse_routes() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .provide(log.clone())
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    let (status, _) = get(&router, "/order/stream", &[("x-deny", "ctrl")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/stream", &[("x-predeny", "ctrlpre")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&router, "/order/stream", &[]).await;
    assert_eq!(status, StatusCode::OK);
    // Hits #1 (denied request) and #2 (this one) — the pre-guard-rejected
    // request never reached the post-auth guard.
    assert!(log.events().contains(&"ctrl@*#2".to_string()));
}

/// AC3: `#[anonymous]` still opts out — the controller's post-auth guard is
/// skipped (no identity, no guard), but its pre-guard still applies.
#[r2e_core::test]
async fn anonymous_skips_controller_guard_but_keeps_pre_guard() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .provide(log.clone())
        .build_state()
        .await
        .register_controller::<AnonController>()
        .build();

    // Anonymous route: 200 without identity, even when the controller guard
    // would deny — it never runs there.
    let (status, body) = get(&router, "/anon/open", &[("x-deny", "cg")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "open");
    assert!(
        !log.events().iter().any(|e| e.starts_with("cg@")),
        "controller post-auth guard must not run on an #[anonymous] route: {:?}",
        log.events()
    );
    // The controller pre-guard DID run (pre-auth is identity-free).
    assert_eq!(log.events(), vec!["pre-cp@*"]);

    // And it can reject the anonymous route.
    let (status, _) = get(&router, "/anon/open", &[("x-predeny", "cp")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The secured route keeps the fail-closed identity + controller guard.
    let (status, _) = get(&router, "/anon/secure", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    log.clear();
    let (status, body) = get(&router, "/anon/secure", &[("x-user", "alice")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "alice");
    assert!(log.events().iter().any(|e| e.starts_with("cg@*")));
    let (status, _) = get(
        &router,
        "/anon/secure",
        &[("x-user", "alice"), ("x-deny", "cg")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
