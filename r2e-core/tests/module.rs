//! Feature modules (`FeatureModule` + `register_module`): closed subgraphs
//! with compile-time encapsulation.
//!
//! Covers: provider registration through `BeanList`, exports-only visibility
//! (private beans absent from the state but constructible from the context),
//! deferred controller registration at `build_state()` (including a
//! controller injecting a **private** module bean), module-to-module wiring
//! via imports/exports, empty controller tuples, and mixing modules with
//! app-level controllers.

use http_body_util::BodyExt;
use r2e_core::beans::BeanRegistry;
use r2e_core::module::{BeanList, FeatureModule};
use r2e_core::prelude::*;
use r2e_core::type_list::{TCons, TNil};
use tower::ServiceExt;

// ── Domain: one imported bean, one private bean, one exported bean ─────────

/// Imported from the app (satisfied via `.provide`).
#[derive(Clone)]
struct DbPool(&'static str);

/// Module-private bean — registered, but NOT exported (absent from `P`).
#[derive(Clone)]
struct UserRepo {
    pool: DbPool,
}

#[bean]
impl UserRepo {
    fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

/// Module-exported bean.
#[derive(Clone)]
struct UserService {
    repo: UserRepo,
}

#[bean]
impl UserService {
    fn new(repo: UserRepo) -> Self {
        Self { repo }
    }
}

/// Depends on the exported bean; the request-scoped field forces the fold to
/// infer a non-trivial extraction-marker witness.
#[controller(path = "/users")]
struct UserModuleController {
    #[inject]
    service: UserService,
    #[inject(request)]
    method: Method,
}

#[routes]
impl UserModuleController {
    #[get("/")]
    async fn list(&self) -> String {
        format!("{} via {}", self.service.repo.pool.0, self.method)
    }
}

/// Depends on the **private** bean — constructs from the retained context
/// even though `UserRepo` is not in the application state.
#[controller(path = "/admin")]
struct AdminController {
    #[inject]
    repo: UserRepo,
}

#[routes]
impl AdminController {
    #[get("/")]
    async fn peek(&self) -> String {
        format!("admin:{}", self.repo.pool.0)
    }
}

struct UserModule;

impl FeatureModule for UserModule {
    type Providers = TCons<UserRepo, TCons<UserService, TNil>>;
    type Controllers = (UserModuleController, AdminController);
    type Exports = TCons<UserService, TNil>;
    type Imports = TCons<DbPool, TNil>;
}

// ── A second module importing the first module's export ────────────────────

#[derive(Clone)]
struct OrderService {
    users: UserService,
}

#[bean]
impl OrderService {
    fn new(users: UserService) -> Self {
        Self { users }
    }
}

#[controller(path = "/orders")]
struct OrderController {
    #[inject]
    orders: OrderService,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> String {
        format!("orders for {}", self.orders.users.repo.pool.0)
    }
}

struct OrderModule;

impl FeatureModule for OrderModule {
    type Providers = TCons<OrderService, TNil>;
    type Controllers = (OrderController,);
    type Exports = TCons<OrderService, TNil>;
    type Imports = TCons<UserService, TNil>;
}

/// A providers-only module: no controllers (`Controllers = ()`).
struct HeadlessModule;

impl FeatureModule for HeadlessModule {
    type Providers = TNil;
    type Controllers = ();
    type Exports = TNil;
    type Imports = TNil;
}

// ── App-level controller consuming a module export ─────────────────────────

#[controller(path = "/app")]
struct AppController {
    #[inject]
    service: UserService,
}

#[routes]
impl AppController {
    #[get("/")]
    async fn index(&self) -> String {
        format!("app:{}", self.service.repo.pool.0)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

trait SameTy<B> {}
impl<A> SameTy<A> for A {}
fn assert_same<A: SameTy<B>, B>() {}

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

// ── Tests ───────────────────────────────────────────────────────────────────

/// `BeanList` derives the provided-types list and the aggregate deps list at
/// the type level.
#[test]
fn bean_list_type_level_fold() {
    type Providers = <UserModule as FeatureModule>::Providers;
    assert_same::<<Providers as BeanList>::Provided, TCons<UserRepo, TCons<UserService, TNil>>>();
    assert_same::<<Providers as BeanList>::Deps, TCons<DbPool, TCons<UserRepo, TNil>>>();
}

/// `BeanList::register_into` registers every provider so the graph resolves
/// them (given the import).
#[r2e_core::test]
async fn bean_list_registers_all_providers() {
    let mut registry = BeanRegistry::new();
    registry.provide(DbPool("mod-db"));
    <<UserModule as FeatureModule>::Providers as BeanList>::register_into(&mut registry);
    let ctx = registry.resolve().await.expect("graph must resolve");
    assert_eq!(ctx.get::<UserService>().repo.pool.0, "mod-db");
    assert_eq!(ctx.get::<UserRepo>().pool.0, "mod-db");
}

/// One `register_module` call registers providers AND controllers; private
/// beans stay out of the state but module controllers still construct.
#[r2e_core::test]
async fn register_module_end_to_end() {
    let app = r2e_core::AppBuilder::new()
        .provide(DbPool("mod-db"))
        .register_module::<UserModule>()
        .build_state()
        .await;

    // Encapsulation at runtime: the private bean is not in the state...
    assert!(app.state().bean::<UserRepo>().is_none());
    assert!(app.state().bean::<UserService>().is_some());
    // ...but remains constructible from the retained context.
    assert_eq!(app.bean_context().get::<UserRepo>().pool.0, "mod-db");

    let router = app.build();
    for (path, expected) in [("/users", "mod-db via GET"), ("/admin", "admin:mod-db")] {
        let (status, body) = get(&router, path).await;
        assert_eq!(status, r2e_core::http::StatusCode::OK, "route {path}");
        assert_eq!(body, expected, "route {path}");
    }
}

/// A module can import another module's export; app-level controllers can
/// consume exports; a controllers-less module is fine.
#[r2e_core::test]
async fn modules_compose_with_each_other_and_app_controllers() {
    let app = r2e_core::AppBuilder::new()
        .provide(DbPool("mod-db"))
        .register_module::<UserModule>()
        .register_module::<OrderModule>()
        .register_module::<HeadlessModule>()
        .build_state()
        .await
        .register_controller::<AppController>();

    let router = app.build();
    for (path, expected) in [
        ("/users", "mod-db via GET"),
        ("/admin", "admin:mod-db"),
        ("/orders", "orders for mod-db"),
        ("/app", "app:mod-db"),
    ] {
        let (status, body) = get(&router, path).await;
        assert_eq!(status, r2e_core::http::StatusCode::OK, "route {path}");
        assert_eq!(body, expected, "route {path}");
    }
}
