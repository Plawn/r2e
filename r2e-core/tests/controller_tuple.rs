//! Coverage for `AppBuilder::register_controllers::<(A, B, C)>()` — the tuple
//! registration form must fan out to the single-controller path for every
//! element, preserving order, so all routes are reachable.

use http_body_util::BodyExt;
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use tower::ServiceExt;

#[derive(Clone)]
struct AppState;

#[controller(state = AppState)]
struct AlphaController;

#[routes]
impl AlphaController {
    #[get("/alpha")]
    async fn handle(&self) -> String {
        "alpha".to_string()
    }
}

#[controller(state = AppState)]
struct BetaController;

#[routes]
impl BetaController {
    #[get("/beta")]
    async fn handle(&self) -> String {
        "beta".to_string()
    }
}

#[controller(state = AppState)]
struct GammaController;

#[routes]
impl GammaController {
    #[get("/gamma")]
    async fn handle(&self) -> String {
        "gamma".to_string()
    }
}

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

/// Registering three controllers via the tuple form must make every route
/// reachable, just as three sequential `register_controller` calls would.
#[r2e_core::test]
async fn register_controllers_tuple_wires_all_routes() {
    let router = r2e_core::AppBuilder::new()
        .with_state(AppState)
        .register_controllers::<(AlphaController, BetaController, GammaController)>()
        .build();

    for (path, expected) in [("/alpha", "alpha"), ("/beta", "beta"), ("/gamma", "gamma")] {
        let (status, body) = get(router.clone(), path).await;
        assert_eq!(status, StatusCode::OK, "route {path} should be reachable");
        assert_eq!(body, expected);
    }
}

/// The single-element tuple form is also supported (arity 1).
#[r2e_core::test]
async fn register_controllers_single_element_tuple() {
    let router = r2e_core::AppBuilder::new()
        .with_state(AppState)
        .register_controllers::<(AlphaController,)>()
        .build();

    let (status, body) = get(router.clone(), "/alpha").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "alpha");
}
