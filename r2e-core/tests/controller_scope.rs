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

struct AppState {
    dependency: CloneTrackedDependency,
}

// Application-state cloning is expected during router construction and
// dispatch. Keep it outside the dependency clone counter so this test only
// observes controller construction.
impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            dependency: CloneTrackedDependency {
                clone_count: Arc::clone(&self.dependency.clone_count),
            },
        }
    }
}

#[controller(state = AppState)]
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
    let clone_count = Arc::new(AtomicUsize::new(0));
    let state = AppState {
        dependency: CloneTrackedDependency {
            clone_count: Arc::clone(&clone_count),
        },
    };

    let router = r2e_core::AppBuilder::new()
        .with_state(state)
        .register_controller::<AppScopedController>()
        .build();
    let clones_after_build = clone_count.load(Ordering::SeqCst);
    assert_eq!(clones_after_build, 1);

    assert_eq!(
        get(router.clone(), "/clone-count", None).await,
        clones_after_build.to_string()
    );
    assert_eq!(
        get(router, "/clone-count", None).await,
        clones_after_build.to_string()
    );
    assert_eq!(clone_count.load(Ordering::SeqCst), clones_after_build);
}

struct RequestIdentity(String);

#[derive(Clone)]
struct IdentityState;

impl FromRequestParts<IdentityState> for RequestIdentity {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &IdentityState,
    ) -> Result<Self, Self::Rejection> {
        let user = parts
            .headers
            .get("x-test-user")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
        Ok(Self(user.to_owned()))
    }
}

#[controller(state = IdentityState)]
struct RequestScopedController {
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

#[r2e_core::test]
async fn struct_identity_controller_remains_request_scoped() {
    let router = r2e_core::AppBuilder::new()
        .with_state(IdentityState)
        .register_controller::<RequestScopedController>()
        .build();

    assert_eq!(
        get(router.clone(), "/identity", Some("alice")).await,
        "alice"
    );
    assert_eq!(get(router, "/identity", Some("bob")).await, "bob");
}
