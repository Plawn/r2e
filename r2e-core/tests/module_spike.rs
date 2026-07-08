//! Phase 5 SPIKE — feature modules (closed subgraphs).
//!
//! Validates the type-level design from `docs/claude/plan-feature-modules.md`
//! before committing to an API (5a). Everything module-shaped in this file is
//! a **local prototype** of what 5a moves into r2e-core proper:
//!
//! - `BeanList`: fold over a `TCons` chain of `Registrable` types, deriving
//!   the provided-types list and the aggregate dependency list.
//! - `FeatureModule`: the declarative trait (Providers / Controllers /
//!   Exports / Imports).
//! - Module-local encapsulation checks (deps ⊆ Provides ∪ Imports,
//!   Exports ⊆ Provides, controller deps ⊆ scope) via `AllSatisfied`.
//! - `ModuleFold` / `ModuleControllers`: the deferred controller registration
//!   that `build_state()` will run in the typed phase, including witness
//!   inference at a call site where the state is the inferred HList.
//!
//! Simulation notes (what the real impl does differently):
//! - Providers are registered here via `.register::<T>()` and the type-level
//!   lists are then rewritten with `with_updated_types` to "exports only in
//!   `P`, imports only in `R`". The real `register_module` registers via
//!   `BeanList::register_into(&mut registry)` and performs a single phantom
//!   rewrite — same end state.
//! - One module controller goes through the checked registration path and one
//!   (with a **private** bean dep) through the unchecked construct+routes
//!   path that 5a will expose internally.

use r2e_core::beans::{BeanRegistry, Registrable};
use r2e_core::prelude::*;
use r2e_core::type_list::{AllSatisfied, TAppend, TCons, TNil};
use r2e_core::{AppBuilder, Controller};
use std::sync::Arc;

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

// ── Module controllers ──────────────────────────────────────────────────────

/// Depends on the exported bean; also carries a request-scoped field so the
/// fold has to infer a non-trivial extraction-marker witness `W`.
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

/// Depends on the **private** bean — must be constructible from the context
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

// ── Prototype: BeanList fold ────────────────────────────────────────────────

trait BeanList {
    /// `TCons` list of each provider's `Registrable::Provided`.
    type Provided;
    /// Concatenation of every provider's `Registrable::Deps`.
    type Deps;
    fn register_into(registry: &mut BeanRegistry);
}

impl BeanList for TNil {
    type Provided = TNil;
    type Deps = TNil;
    fn register_into(_registry: &mut BeanRegistry) {}
}

impl<H: Registrable, T: BeanList> BeanList for TCons<H, T>
where
    H::Deps: TAppend<T::Deps>,
{
    type Provided = TCons<H::Provided, T::Provided>;
    type Deps = <H::Deps as TAppend<T::Deps>>::Output;
    fn register_into(registry: &mut BeanRegistry) {
        H::register_into(registry);
        T::register_into(registry);
    }
}

// ── Prototype: FeatureModule + encapsulation checks ─────────────────────────

trait FeatureModule {
    /// `TCons` list of `Registrable` provider types.
    type Providers;
    /// Tuple of controller types.
    type Controllers;
    /// `TCons` list of bean types leaked to the app-global provision list.
    type Exports;
    /// `TCons` list of bean types required from outside the module.
    type Imports;
}

struct UserModule;

impl FeatureModule for UserModule {
    type Providers = TCons<UserRepo, TCons<UserService, TNil>>;
    type Controllers = (UserModuleController, AdminController);
    type Exports = TCons<UserService, TNil>;
    type Imports = TCons<DbPool, TNil>;
}

/// Aggregate `ContextConstruct::Deps` over a controller tuple (arity 2 is
/// enough for the spike; the real impl macro-generates 0..=16).
trait ControllerDepsList {
    type Deps;
}

impl<C0, C1> ControllerDepsList for (C0, C1)
where
    C0: ContextConstruct,
    C1: ContextConstruct,
    C0::Deps: TAppend<C1::Deps>,
{
    type Deps = <C0::Deps as TAppend<C1::Deps>>::Output;
}

/// The module's local resolution scope: Provides ∪ Imports.
type LocalScope<M> = <<<M as FeatureModule>::Providers as BeanList>::Provided as TAppend<
    <M as FeatureModule>::Imports,
>>::Output;

/// Compile-time encapsulation: provider deps and controller deps must resolve
/// inside the module scope; exports must be a subset of what the module
/// provides. (5b gives these dedicated traits with targeted diagnostics —
/// the spike reuses `AllSatisfied`.)
fn assert_module_encapsulated<M, W1, W2, W3>()
where
    M: FeatureModule,
    M::Providers: BeanList,
    <M::Providers as BeanList>::Provided: TAppend<M::Imports>,
    <M::Providers as BeanList>::Deps: AllSatisfied<LocalScope<M>, W1>,
    M::Exports: AllSatisfied<<M::Providers as BeanList>::Provided, W2>,
    M::Controllers: ControllerDepsList,
    <M::Controllers as ControllerDepsList>::Deps: AllSatisfied<LocalScope<M>, W3>,
{
}

// ── Prototype: deferred controller registration (the build_state fold) ─────

trait ModuleControllers<T: Clone + Send + Sync + 'static, W> {
    fn register_all(builder: AppBuilder<T>) -> AppBuilder<T>;
}

/// Arity-2 impl: element 0 exercises the checked path (deps in state),
/// element 1 the unchecked path (deps checked module-locally; construct from
/// the retained bean context, merge routes).
impl<T, C0, W0, D0, C1, W1> ModuleControllers<T, ((W0, D0), (W1,))> for (C0, C1)
where
    T: Clone + Send + Sync + 'static,
    C0: Controller<T, W0>,
    C0::Deps: AllSatisfied<T, D0>,
    C1: Controller<T, W1>,
{
    fn register_all(builder: AppBuilder<T>) -> AppBuilder<T> {
        let builder = builder.register_controller_impl::<C0, W0, D0>();
        let fragment = {
            let core = Arc::new(C1::construct(builder.state(), builder.bean_context()));
            C1::routes(builder.state(), core)
        };
        builder.merge_router(fragment)
    }
}

trait ModuleFold<T: Clone + Send + Sync + 'static, W> {
    fn apply(builder: AppBuilder<T>) -> AppBuilder<T>;
}

impl<T: Clone + Send + Sync + 'static> ModuleFold<T, ()> for TNil {
    fn apply(builder: AppBuilder<T>) -> AppBuilder<T> {
        builder
    }
}

impl<T, M, Rest, WC, WR> ModuleFold<T, (WC, WR)> for TCons<M, Rest>
where
    T: Clone + Send + Sync + 'static,
    M: FeatureModule,
    M::Controllers: ModuleControllers<T, WC>,
    Rest: ModuleFold<T, WR>,
{
    fn apply(builder: AppBuilder<T>) -> AppBuilder<T> {
        Rest::apply(<M::Controllers as ModuleControllers<T, WC>>::register_all(builder))
    }
}

/// Stand-in for the fold `build_state()` will run: `T` is the inferred HList
/// state and `W` must be inferred at the call site.
fn apply_modules<Mods, T, W>(builder: AppBuilder<T>) -> AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
    Mods: ModuleFold<T, W>,
{
    Mods::apply(builder)
}

// ── Type-equality helper ────────────────────────────────────────────────────

trait SameTy<B> {}
impl<A> SameTy<A> for A {}
fn assert_same<A: SameTy<B>, B>() {}

// ── Tests ───────────────────────────────────────────────────────────────────

/// s1 — `BeanList` derives the provided-types list and the aggregate deps
/// list at the type level.
#[test]
fn bean_list_type_level_fold() {
    type Providers = <UserModule as FeatureModule>::Providers;
    assert_same::<<Providers as BeanList>::Provided, TCons<UserRepo, TCons<UserService, TNil>>>();
    assert_same::<<Providers as BeanList>::Deps, TCons<DbPool, TCons<UserRepo, TNil>>>();
}

/// s1 — `BeanList::register_into` registers every provider so the graph
/// resolves them (given the import).
#[r2e_core::test]
async fn bean_list_registers_all_providers() {
    let mut registry = BeanRegistry::new();
    registry.provide(DbPool("spike-db"));
    <<UserModule as FeatureModule>::Providers as BeanList>::register_into(&mut registry);
    let ctx = registry.resolve().await.expect("graph must resolve");
    assert_eq!(ctx.get::<UserService>().repo.pool.0, "spike-db");
    assert_eq!(ctx.get::<UserRepo>().pool.0, "spike-db");
}

/// s2 — the encapsulation bounds hold for a well-formed module, with all
/// witnesses inferred at the call site.
#[test]
fn module_encapsulation_checks_hold() {
    assert_module_encapsulated::<UserModule, _, _, _>();
}

/// s3 + s4 — end to end: exports-only `P`, imports-only `R`, private bean
/// invisible in the state but present in the context, and the deferred
/// controller fold registering both module controllers (checked + unchecked
/// paths) with inferred witnesses.
#[r2e_core::test]
async fn module_end_to_end() {
    // Simulated register_module::<UserModule>(): runtime registers ALL
    // providers; the phantom rewrite keeps only Exports in P and only
    // Imports in R (provider-internal deps are checked module-locally, NOT
    // appended to the global requirement list).
    let builder = AppBuilder::new()
        .provide(DbPool("spike-db"))
        .register::<UserRepo>()
        .register::<UserService>()
        .with_updated_types::<
            TCons<UserService, TCons<DbPool, TNil>>, // P = Exports ++ [DbPool]
            TCons<DbPool, TNil>,                     // R = Imports
        >();

    let app = builder.build_state().await;

    // Encapsulation at runtime: the private bean is not in the state...
    assert!(app.state().bean::<UserRepo>().is_none());
    assert!(app.state().bean::<UserService>().is_some());
    // ...but remains constructible from the retained context.
    assert_eq!(app.bean_context().get::<UserRepo>().pool.0, "spike-db");

    // Deferred controller registration (what build_state will do in 5a).
    let app = apply_modules::<TCons<UserModule, TNil>, _, _>(app);
    let router = app.build();

    use http_body_util::BodyExt;
    use tower::ServiceExt;
    for (path, expected) in [("/users", "spike-db via GET"), ("/admin", "admin:spike-db")] {
        let req = r2e_core::http::Request::builder()
            .uri(path)
            .body(r2e_core::http::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), r2e_core::http::StatusCode::OK, "route {path}");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), expected, "route {path}");
    }
}

