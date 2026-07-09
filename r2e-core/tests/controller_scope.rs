use http_body_util::BodyExt;
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

async fn get(router: r2e_core::http::Router, path: &str, user: Option<&str>) -> String {
    let mut request = Request::builder().uri(path);
    if let Some(user) = user {
        request = request.header("x-test-user", user);
    }
    let response = router
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

struct CloneTrackedDependency {
    clone_count: Arc<AtomicUsize>,
}

impl Clone for CloneTrackedDependency {
    fn clone(&self) -> Self {
        self.clone_count.fetch_add(1, Ordering::SeqCst);
        Self {
            clone_count: Arc::clone(&self.clone_count),
        }
    }
}

/// A structurally identical probe that no controller injects.
///
/// In the HList application-state model every provided bean is a member of the
/// state, so a controller's injected dependency is cloned whenever the state is
/// cloned per request. This probe lives in the same state and absorbs that
/// identical per-request cloning, but is never pulled by a core's
/// `from_context`. The difference between the two counters therefore isolates
/// core (re)construction from routine per-request state cloning.
struct StateOnlyProbe {
    clone_count: Arc<AtomicUsize>,
}

impl Clone for StateOnlyProbe {
    fn clone(&self) -> Self {
        self.clone_count.fetch_add(1, Ordering::SeqCst);
        Self {
            clone_count: Arc::clone(&self.clone_count),
        }
    }
}

#[controller]
struct AppScopedController {
    #[inject]
    dependency: CloneTrackedDependency,
}

#[routes]
impl AppScopedController {
    #[get("/clone-count")]
    async fn clone_count(&self) -> String {
        self.dependency
            .clone_count
            .load(Ordering::SeqCst)
            .to_string()
    }
}

#[r2e_core::test]
async fn standard_controller_is_constructed_once_and_reused() {
    let core_count = Arc::new(AtomicUsize::new(0));
    let state_count = Arc::new(AtomicUsize::new(0));

    let router = r2e_core::AppBuilder::new()
        .provide(CloneTrackedDependency {
            clone_count: Arc::clone(&core_count),
        })
        .provide(StateOnlyProbe {
            clone_count: Arc::clone(&state_count),
        })
        .build_state()
        .await
        .register_controller::<AppScopedController>()
        .build();

    // Baselines after the single build-time construction. The injected
    // dependency is additionally cloned once by the core's `from_context`, so
    // `core_count` may exceed `state_count` by a fixed amount we subtract out.
    let core_base = core_count.load(Ordering::SeqCst);
    let state_base = state_count.load(Ordering::SeqCst);

    for _ in 0..3 {
        let _ = get(router.clone(), "/clone-count", None).await;
    }

    // Both beans absorb identical per-request state cloning, so their growth
    // must match exactly: the core is built once and reused, never rebuilt per
    // request (which would clone the injected dependency again via
    // `from_context`, diverging the two counters).
    let core_delta = core_count.load(Ordering::SeqCst) - core_base;
    let state_delta = state_count.load(Ordering::SeqCst) - state_base;
    assert_eq!(
        core_delta, state_delta,
        "standard controller core must be built once and reused, not rebuilt per request \
         (injected dep clones grew by {core_delta}, state-only probe by {state_delta})"
    );
}

struct RequestIdentity(String);

// `from_state` clones this dependency into the controller core, so the
// dependency clone counter is a proxy for the number of times the core was
// constructed. With the request façade, a struct-identity controller's core is
// built once at router-build time and never per request.
struct CoreBuildTracked {
    clones: Arc<AtomicUsize>,
}

impl Clone for CoreBuildTracked {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestIdentity {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let user = parts
            .headers
            .get("x-test-user")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
        Ok(Self(user.to_owned()))
    }
}

#[controller]
struct RequestScopedController {
    #[inject]
    #[allow(dead_code)]
    dep: CoreBuildTracked,
    #[inject(identity)]
    identity: RequestIdentity,
}

#[routes]
impl RequestScopedController {
    #[get("/identity")]
    async fn identity(&self) -> String {
        self.identity.0.clone()
    }
}

/// Struct-level identity no longer reconstructs the controller core per request:
/// the core is built exactly once (router-build time) and each request only
/// extracts a fresh identity into a stack façade.
#[r2e_core::test]
async fn struct_identity_controller_core_built_once() {
    let core_count = Arc::new(AtomicUsize::new(0));
    let state_count = Arc::new(AtomicUsize::new(0));

    let router = r2e_core::AppBuilder::new()
        .provide(CoreBuildTracked {
            clones: Arc::clone(&core_count),
        })
        .provide(StateOnlyProbe {
            clone_count: Arc::clone(&state_count),
        })
        .build_state()
        .await
        .register_controller::<RequestScopedController>()
        .build();

    let core_base = core_count.load(Ordering::SeqCst);
    let state_base = state_count.load(Ordering::SeqCst);

    // Each request still sees its own extracted identity...
    assert_eq!(
        get(router.clone(), "/identity", Some("alice")).await,
        "alice"
    );
    assert_eq!(get(router.clone(), "/identity", Some("bob")).await, "bob");
    for _ in 0..5 {
        let _ = get(router.clone(), "/identity", Some("carol")).await;
    }

    // ...but the core was never rebuilt: the injected dependency and the
    // state-only probe absorb identical per-request state cloning, so their
    // growth must match. A per-request core rebuild would clone the dependency
    // again via `from_context` and diverge the two counters.
    let core_delta = core_count.load(Ordering::SeqCst) - core_base;
    let state_delta = state_count.load(Ordering::SeqCst) - state_base;
    assert_eq!(
        core_delta, state_delta,
        "struct-identity controller core must be built once, not rebuilt per request \
         (injected dep clones grew by {core_delta}, state-only probe by {state_delta})"
    );
}
