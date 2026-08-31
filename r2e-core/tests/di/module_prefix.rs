//! `#[module(prefix = "/api/v1")]` — a feature module mounts its controllers
//! under a path prefix.
//!
//! The prefix is declared on the module (not at the `register_module` call
//! site) so it composes through aggregates, which are purely static folds. It
//! must reach three places: the served router, the collected `RouteInfo`
//! (what OpenAPI publishes), and — textually, in the CLI — `r2e routes`.

use http_body_util::BodyExt;
use r2e_core::di::meta::RouteInfo;
use r2e_core::di::module::FeatureModule;
use r2e_core::http::extract::Path;
use r2e_core::prelude::*;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Clone)]
struct Catalog(&'static str);

// ── v1: a module mounted under /api/v1 ─────────────────────────────────────

#[controller(path = "/items")]
struct V1ItemsController {
    #[inject]
    catalog: Catalog,
}

#[routes]
impl V1ItemsController {
    #[get("/")]
    async fn list(&self) -> String {
        format!("v1 items of {}", self.catalog.0)
    }

    #[get("/{id}")]
    async fn one(&self, Path(id): Path<String>) -> String {
        format!("v1 item {id}")
    }
}

#[module(prefix = "/api/v1", controllers(V1ItemsController), imports(Catalog))]
struct V1Module;

// ── v2: same shape, a different mount point ────────────────────────────────

#[controller(path = "/items")]
struct V2ItemsController {
    #[inject]
    catalog: Catalog,
}

#[routes]
impl V2ItemsController {
    #[get("/")]
    async fn list(&self) -> String {
        format!("v2 items of {}", self.catalog.0)
    }
}

#[module(prefix = "/api/v2", controllers(V2ItemsController), imports(Catalog))]
struct V2Module;

// ── An unprefixed module keeps mounting at the app root ────────────────────

#[controller(path = "/health")]
struct HealthController;

#[routes]
impl HealthController {
    #[get("/")]
    async fn health(&self) -> String {
        "ok".to_string()
    }
}

#[module(controllers(HealthController))]
struct HealthModule;

// An aggregate owns no controllers: each member keeps its own prefix.
#[module(modules(V1Module, V2Module, HealthModule))]
struct ApiModules;

// ── Helpers ────────────────────────────────────────────────────────────────

async fn get(router: &r2e_core::http::Router, path: &str) -> (r2e_core::http::StatusCode, String) {
    let req = r2e_core::http::Request::builder()
        .uri(path)
        .body(r2e_core::http::Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn prefix_is_a_const_on_the_module() {
    assert_eq!(<V1Module as FeatureModule>::PATH_PREFIX, Some("/api/v1"));
    assert_eq!(<HealthModule as FeatureModule>::PATH_PREFIX, None);
}

#[r2e_core::test]
async fn module_routes_are_served_under_the_prefix() {
    let router = r2e_core::AppBuilder::new()
        .provide(Catalog("main"))
        .register_module::<V1Module>()
        .build_state()
        .await
        .build();

    let (status, body) = get(&router, "/api/v1/items").await;
    assert_eq!(status, r2e_core::http::StatusCode::OK);
    assert_eq!(body, "v1 items of main");

    let (status, body) = get(&router, "/api/v1/items/42").await;
    assert_eq!(status, r2e_core::http::StatusCode::OK);
    assert_eq!(body, "v1 item 42");

    // The controller's own path is no longer mounted at the root.
    let (status, _) = get(&router, "/items").await;
    assert_eq!(status, r2e_core::http::StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn aggregate_members_keep_their_own_prefixes() {
    let router = r2e_core::AppBuilder::new()
        .provide(Catalog("main"))
        .register_modules::<ApiModules>()
        .build_state()
        .await
        .build();

    for (path, expected) in [
        ("/api/v1/items", "v1 items of main"),
        ("/api/v2/items", "v2 items of main"),
        // An unprefixed member in the same aggregate still mounts at the root.
        ("/health", "ok"),
    ] {
        let (status, body) = get(&router, path).await;
        assert_eq!(status, r2e_core::http::StatusCode::OK, "route {path}");
        assert_eq!(body, expected, "route {path}");
    }
}

#[r2e_core::test]
async fn collected_route_metadata_carries_the_mounted_path() {
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);

    let _router = r2e_core::AppBuilder::new()
        .provide(Catalog("main"))
        .register_modules::<ApiModules>()
        .build_state()
        .await
        .with_meta_consumer::<RouteInfo, _>(move |items| {
            *sink.lock().unwrap() = items.iter().map(|i| i.path.clone()).collect();
            r2e_core::http::Router::new()
        })
        .build();

    let paths = captured.lock().unwrap().clone();
    // What OpenAPI publishes must be what the router serves.
    assert!(
        paths.iter().any(|p| p.starts_with("/api/v1/items")),
        "paths: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with("/api/v2/items")),
        "paths: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with("/health")),
        "paths: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("/items")),
        "an unprefixed leftover would publish a route nobody serves: {paths:?}"
    );
}
