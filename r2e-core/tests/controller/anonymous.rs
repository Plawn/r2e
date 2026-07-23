//! `#[anonymous]`: per-route opt-out on a fail-closed identity controller.
//!
//! Anonymous routes are emitted on the controller **core** — identity
//! extraction is skipped entirely, guards still run (with `identity: None`),
//! and an `Option<T>` identity parameter makes the route adaptive.

use r2e_core::http::response::Response;
use r2e_core::http::StatusCode;
use r2e_core::prelude::*;
use r2e_core::{Guard, GuardContext};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::fixtures::{req, FlaggingId, Subject};

// ── 13. #[anonymous]: per-route opt-out on an identity controller ───────────

#[controller]
struct AnonMixedController {
    #[inject]
    label: String,
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl AnonMixedController {
    /// Public — runs on the core: no identity extraction, injected fields
    /// reachable directly (`self.user` here would be a compile error).
    #[get("/anon/pub")]
    #[anonymous]
    async fn public(&self) -> String {
        format!("pub:{}", self.label)
    }

    /// Authenticated by default (fail-closed) — reads the struct identity.
    #[get("/anon/priv")]
    async fn private(&self) -> String {
        format!("priv:{}", self.user.0)
    }
}

#[r2e_core::test]
async fn anonymous_route_is_public_and_others_stay_authed() {
    let router = r2e_core::AppBuilder::new()
        .provide("core".to_string())
        .build_state()
        .await
        .register_controller::<AnonMixedController>()
        .build();

    // Anonymous route: 200 with no credentials, core field reachable.
    let (status, body) = req(router.clone(), "/anon/pub", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "pub:core");

    // Credentials on an anonymous route are simply ignored.
    let (status, _) = req(router.clone(), "/anon/pub", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);

    // Every unmarked route stays authenticated.
    let (status, _) = req(router.clone(), "/anon/priv", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = req(router, "/anon/priv", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "priv:zoe");
}

// ── 14. #[anonymous] skips identity extraction entirely ─────────────────────

#[controller]
struct AnonFlagController {
    #[inject(identity)]
    user: FlaggingId,
}

#[routes]
impl AnonFlagController {
    #[get("/anon/flag/pub")]
    #[anonymous]
    async fn public(&self) -> &'static str {
        "ok"
    }

    #[get("/anon/flag/priv")]
    async fn private(&self) -> String {
        self.user.0.clone()
    }
}

#[r2e_core::test]
async fn anonymous_route_skips_identity_extraction() {
    let identity_ran = Arc::new(AtomicBool::new(false));
    let router = r2e_core::AppBuilder::new()
        .provide(identity_ran.clone())
        .build_state()
        .await
        .register_controller::<AnonFlagController>()
        .build();

    // Even with credentials present, the anonymous route never runs the
    // identity extractor — no JWT-validation cost on public routes.
    let (status, _) = req(router.clone(), "/anon/flag/pub", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !identity_ran.load(Ordering::SeqCst),
        "anonymous route must not run identity extraction"
    );

    // The extractor still runs for unmarked routes.
    let (status, _) = req(router, "/anon/flag/priv", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(identity_ran.load(Ordering::SeqCst));
}

// ── 15. Guards on #[anonymous] routes run with identity: None ────────────────

struct AnonProbe;

struct AnonProbeReady {
    saw: Arc<Mutex<Vec<String>>>,
}

impl r2e_core::DecoratorSpec for AnonProbe {
    type Product = AnonProbeReady;
    type Deps = r2e_core::type_list::TCons<Arc<Mutex<Vec<String>>>, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> AnonProbeReady {
        AnonProbeReady { saw: ctx.get() }
    }
}

impl Guard<Subject> for AnonProbeReady {
    fn check(
        &self,
        ctx: &GuardContext<'_, Subject>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let seen = if ctx.identity.is_some() {
            "some"
        } else {
            "none"
        };
        self.saw.lock().unwrap().push(seen.to_string());
        std::future::ready(Ok(()))
    }
}

#[controller]
struct AnonGuardController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl AnonGuardController {
    /// Non-identity guards (rate limiting, IP checks, …) still run on
    /// anonymous routes — with `identity: None`.
    #[get("/anon/guarded")]
    #[anonymous]
    #[guard(AnonProbe)]
    async fn show(&self) -> &'static str {
        "guarded"
    }
}

#[r2e_core::test]
async fn guard_on_anonymous_route_sees_no_identity() {
    let saw: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let router = r2e_core::AppBuilder::new()
        .provide(saw.clone())
        .build_state()
        .await
        .register_controller::<AnonGuardController>()
        .build();

    let (status, body) = req(router, "/anon/guarded", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "guarded");
    assert_eq!(
        *saw.lock().unwrap(),
        vec!["none"],
        "the guard must run exactly once, with no identity"
    );
}

// ── 16. Adaptive route: #[anonymous] + optional identity parameter ──────────

#[controller]
struct AnonAdaptiveController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl AnonAdaptiveController {
    /// Public, but personalized when a credential is present: the optional
    /// identity parameter is its own extraction, independent of the (skipped)
    /// struct identity.
    #[get("/anon/adaptive")]
    #[anonymous]
    async fn show(&self, #[inject(identity)] user: Option<Subject>) -> String {
        match user {
            Some(u) => format!("hello:{}", u.0),
            None => "hello:stranger".to_string(),
        }
    }
}

#[r2e_core::test]
async fn anonymous_route_with_optional_identity_param_adapts() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<AnonAdaptiveController>()
        .build();

    let (status, body) = req(router.clone(), "/anon/adaptive", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello:stranger");

    let (status, body) = req(router, "/anon/adaptive", Some("zoe"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello:zoe");
}
