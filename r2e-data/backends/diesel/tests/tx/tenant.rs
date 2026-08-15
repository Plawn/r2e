//! Per-tenant transactions, end to end (feature `tenant`).
//!
//! Each tenant here is a *separate SQLite file*, so "the right tenant's
//! database" is not an assertion about a label carried around — a row written
//! for `acme` is physically absent from `globex`'s database. The rest of the
//! module pins the things that only show up on the request path: the
//! commit/rollback boundary per tenant, the tenant a transaction reports, and
//! the two cases where **no pool must be opened at all** (an unknown tenant, a
//! rejected guard).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use diesel::r2d2::{ConnectionManager, Pool};
use diesel::{sql_query, QueryableByName, RunQueryDsl, SqliteConnection};
use http_body_util::BodyExt;
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Method, Parts, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::request_head::RequestHead;
use r2e_core::{AppBuilder, GuardContext, Identity};
use r2e_data_diesel::{PoolSource, TenantPools, TenantTx};
use r2e_tenant::{
    HeaderTenantResolver, SyncTenantResolver, Tenancy, TenantId, TenantResolver, TenantRouter,
    TenantedSettings,
};
use tower::ServiceExt;

use crate::support::{cleanup_sqlite_file, sqlite_file_path};

const CREATE_ITEMS: &str = "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)";

#[derive(QueryableByName)]
struct Name {
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
}

// ── controllers ─────────────────────────────────────────────────────────────

/// The headline shape: a `#[managed]` transaction and nothing else — no tenant
/// field, no bean field. Everything per-tenant comes from the request.
#[controller(path = "/items")]
struct ItemController;

#[routes]
impl ItemController {
    #[post("/")]
    async fn create(
        &self,
        #[managed] tx: &mut TenantTx<SqliteConnection>,
    ) -> Result<String, HttpError> {
        let tenant = tx.tenant().as_str().to_string();
        let inserted = tenant.clone();
        tx.run(move |connection| {
            sql_query("INSERT INTO items(name) VALUES (?)")
                .bind::<diesel::sql_types::Text, _>(inserted)
                .execute(connection)
        })
        .await?;
        Ok(tenant)
    }

    #[get("/")]
    async fn list(
        &self,
        #[managed] tx: &mut TenantTx<SqliteConnection>,
    ) -> Result<String, HttpError> {
        let names = tx
            .run(|connection| {
                sql_query("SELECT name FROM items ORDER BY id").load::<Name>(connection)
            })
            .await?;
        Ok(names
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>()
            .join(","))
    }

    /// Writes, then fails: the write must not survive the response.
    #[post("/fail")]
    async fn create_then_fail(
        &self,
        #[managed] tx: &mut TenantTx<SqliteConnection>,
    ) -> Result<String, HttpError> {
        tx.run(|connection| {
            sql_query("INSERT INTO items(name) VALUES ('rolled back')").execute(connection)
        })
        .await?;
        Err(HttpError::bad_request("boom"))
    }
}

/// A route that *also* extracts the tenant: the extractor memoizes it, so the
/// transaction must reuse that answer instead of resolving a second time.
#[controller(path = "/memo")]
struct MemoController {
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl MemoController {
    #[get("/")]
    async fn whoami(&self, #[managed] tx: &mut TenantTx<SqliteConnection>) -> String {
        format!("{}/{}", self.tenant, tx.tenant())
    }
}

/// A guard that always rejects — the cheapest way to ask "did anything get
/// opened before the guard said no?".
struct Deny(&'static str);

impl SelfBuilt for Deny {}

impl<I: Identity> Guard<I> for Deny {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let reason = self.0;
        async move { Err(GuardError::forbidden(reason).into()) }
    }
}

#[controller(path = "/guarded")]
struct GuardedController;

#[routes]
impl GuardedController {
    #[get("/")]
    #[guard(Deny("nope"))]
    async fn blocked(&self, #[managed] tx: &mut TenantTx<SqliteConnection>) -> String {
        tx.tenant().as_str().to_string()
    }
}

/// Two transactions on one request. Nothing here extracts the tenant, so the
/// only thing that can keep both on the same database is the request's
/// resolve-once cell.
#[controller(path = "/twin")]
struct TwinController;

#[routes]
impl TwinController {
    #[get("/")]
    async fn twin(
        &self,
        #[managed] left: &mut TenantTx<SqliteConnection>,
        #[managed] right: &mut TenantTx<SqliteConnection>,
    ) -> Result<String, HttpError> {
        // Read through both, so "same tenant" is a claim about the databases
        // reached and not only about the labels the transactions carry.
        let left_tenant = left.tenant().as_str().to_string();
        let right_tenant = right.tenant().as_str().to_string();
        let left_rows = left
            .run(|connection| sql_query("SELECT name FROM items").load::<Name>(connection))
            .await?;
        let right_rows = right
            .run(|connection| sql_query("SELECT name FROM items").load::<Name>(connection))
            .await?;
        Ok(format!(
            "{}:{}/{}:{}",
            left_tenant,
            left_rows.len(),
            right_tenant,
            right_rows.len()
        ))
    }
}

/// A guard that resolves the tenant itself — the earliest anything can, since
/// guards run before `#[managed]` acquisition. The transaction that follows
/// must land on the guard's answer, not on a second resolution.
#[derive(DecoratorBean)]
struct SeeTenant {
    #[inject]
    router: TenantRouter,
}

impl<I: Identity> Guard<I> for SeeTenant {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            self.router
                .resolve(&ctx.head())
                .await
                .map(|_| ())
                .map_err(r2e_core::http::response::IntoResponse::into_response)
        }
    }
}

#[controller(path = "/guard-first")]
struct GuardFirstController;

#[routes]
impl GuardFirstController {
    #[get("/")]
    #[guard(SeeTenant::spec())]
    async fn go(&self, #[managed] tx: &mut TenantTx<SqliteConnection>) -> String {
        tx.tenant().as_str().to_string()
    }
}

// ── the tenant as a JWT claim ───────────────────────────────────────────────

/// What an authentication extractor leaves behind for the resolver to project.
#[derive(Clone)]
struct TenantClaim(String);

/// A stand-in for `AuthenticatedUser`: reads `authorization: Bearer <sub>@<tenant>`
/// and parks the tenant claim in the request extensions.
#[derive(Clone)]
struct ClaimUser {
    sub: String,
}

impl Identity for ClaimUser {
    fn sub(&self) -> &str {
        &self.sub
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ClaimUser {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| HttpError::unauthorized("no token"))?;
        let (sub, tenant) = token
            .split_once('@')
            .ok_or_else(|| HttpError::unauthorized("token carries no tenant"))?;
        parts.extensions.insert(TenantClaim(tenant.to_string()));
        Ok(Self {
            sub: sub.to_string(),
        })
    }
}

/// The `ExtensionTenantResolver` shape, as a named type so it can be the
/// `Tenancy` plugin's resolver bean: project whatever authentication parked.
#[derive(Clone, Default)]
struct ClaimResolver;

impl SyncTenantResolver for ClaimResolver {
    fn resolve_sync(&self, head: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        head.extension::<TenantClaim>()
            .map(|claim| TenantId::parse(&claim.0))
            .transpose()
            .map_err(|error| HttpError::bad_request(error.to_string()))
    }
}

/// The tenant comes from the identity, and the identity is a controller field —
/// request-scoped fields are extracted in declaration order, so the claim is
/// parked before anything resolves.
#[controller(path = "/jwt-struct")]
struct JwtStructController {
    #[inject(identity)]
    user: ClaimUser,
}

#[routes]
impl JwtStructController {
    #[get("/")]
    async fn who(&self, #[managed] tx: &mut TenantTx<SqliteConnection>) -> String {
        format!("{}@{}", self.user.sub, tx.tenant())
    }
}

/// The same, with the identity declared on the handler instead: the generated
/// closure must extract it **before** it snapshots the head the `#[managed]`
/// resource resolves from.
#[controller(path = "/jwt-param")]
struct JwtParamController;

#[routes]
impl JwtParamController {
    #[get("/")]
    async fn who(
        &self,
        #[inject(identity)] user: ClaimUser,
        #[managed] tx: &mut TenantTx<SqliteConnection>,
    ) -> String {
        format!("{}@{}", user.sub, tx.tenant())
    }
}

// ── fixtures ────────────────────────────────────────────────────────────────

/// A tenant directory: two provisioned tenants with their own database file,
/// and a counter for how often the DSN lookup ran.
struct Directory {
    dsns: HashMap<String, String>,
    lookups: Arc<AtomicUsize>,
}

impl Directory {
    /// Provision `acme` and `globex` (each an empty `items` table in its own
    /// file). Every other tenant — `ghost` in these tests — is unknown.
    fn provision() -> Self {
        let mut dsns = HashMap::new();
        for tenant in ["acme", "globex"] {
            let path = sqlite_file_path(tenant);
            let pool = Pool::builder()
                .max_size(1)
                .build(ConnectionManager::<SqliteConnection>::new(&path))
                .unwrap();
            sql_query(CREATE_ITEMS)
                .execute(&mut pool.get().unwrap())
                .unwrap();
            dsns.insert(tenant.to_string(), path);
        }
        Self {
            dsns,
            lookups: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn source(&self) -> PoolSource<SqliteConnection> {
        let dsns = self.dsns.clone();
        let lookups = Arc::clone(&self.lookups);
        PoolSource::<SqliteConnection>::sync(move |tenant: &TenantId| {
            lookups.fetch_add(1, Ordering::SeqCst);
            dsns.get(tenant.as_str()).cloned()
        })
        // Two: `/twin` holds two transactions of the same tenant open at once.
        .max_connections(2)
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }

    fn cleanup(&self) {
        for path in self.dsns.values() {
            cleanup_sqlite_file(path);
        }
    }
}

/// A resolver that counts its calls and answers a *different* tenant every
/// time. Anything resolving twice within one request therefore ends up on two
/// databases, which is exactly what these tests are looking for.
#[derive(Clone)]
struct Alternating {
    calls: Arc<AtomicUsize>,
    answers: Vec<&'static str>,
}

impl Alternating {
    fn new(answers: &[&'static str]) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            answers: answers.to_vec(),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SyncTenantResolver for Alternating {
    fn resolve_sync(&self, _head: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        let nth = self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers[nth % self.answers.len()];
        Ok(Some(TenantId::parse(answer).unwrap()))
    }
}

/// The whole app: a resolver bean behind the [`Tenancy`] plugin (which is what
/// installs the per-request resolve-once cell), a per-tenant pool map over the
/// directory, and every controller. The map is returned too, so tests can ask
/// which tenants actually got a pool.
async fn app_with<R>(directory: &Directory, resolver: R) -> (Router, TenantPools<SqliteConnection>)
where
    R: TenantResolver + Clone + Send + Sync + 'static,
{
    let pools = TenantPools::<SqliteConnection>::new(
        Arc::new(directory.source()),
        r2e_core::plugin::GraphHandle::default(),
        TenantedSettings::default(),
        None,
    );
    let router = AppBuilder::new()
        .provide(resolver)
        .plugin(Tenancy::resolver::<R>().require_tenant())
        .provide(pools.clone())
        .build_state()
        .await
        .register_controller::<ItemController>()
        .register_controller::<MemoController>()
        .register_controller::<GuardedController>()
        .register_controller::<TwinController>()
        .register_controller::<GuardFirstController>()
        .register_controller::<JwtStructController>()
        .register_controller::<JwtParamController>()
        .build();
    (router, pools)
}

/// The common case: `x-tenant-id`, requests without a tenant rejected.
async fn app(directory: &Directory) -> (Router, TenantPools<SqliteConnection>) {
    app_with(directory, HeaderTenantResolver::default()).await
}

/// Drive one request, optionally naming a tenant.
async fn send(
    router: &Router,
    method: Method,
    path: &str,
    tenant: Option<&str>,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(tenant) = tenant {
        request = request.header("x-tenant-id", tenant);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(router: &Router, path: &str, tenant: &str) -> (StatusCode, String) {
    send(router, Method::POST, path, Some(tenant)).await
}

async fn get(router: &Router, path: &str, tenant: &str) -> (StatusCode, String) {
    send(router, Method::GET, path, Some(tenant)).await
}

// ── tests ───────────────────────────────────────────────────────────────────

/// The isolation claim, in physical terms: writes for one tenant are invisible
/// in the other tenant's database.
#[tokio::test]
async fn each_tenant_writes_to_its_own_database() {
    let directory = Directory::provision();
    let (router, pools) = app(&directory).await;

    assert_eq!(post(&router, "/items", "acme").await.0, StatusCode::OK);
    assert_eq!(post(&router, "/items", "acme").await.0, StatusCode::OK);
    assert_eq!(post(&router, "/items", "globex").await.0, StatusCode::OK);

    let (status, acme) = get(&router, "/items", "acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(acme, "acme,acme");
    let (_, globex) = get(&router, "/items", "globex").await;
    assert_eq!(globex, "globex");

    let mut active: Vec<String> = pools
        .active()
        .iter()
        .map(|tenant| tenant.as_str().to_string())
        .collect();
    active.sort();
    assert_eq!(active, ["acme", "globex"]);
    assert_eq!(
        directory.lookups(),
        2,
        "one pool per tenant, not one per request"
    );

    directory.cleanup();
}

/// Commit on success, rollback on failure — and the rollback is scoped to the
/// tenant whose request failed.
#[tokio::test]
async fn a_failing_handler_rolls_back_only_its_own_tenant() {
    let directory = Directory::provision();
    let (router, _pools) = app(&directory).await;

    post(&router, "/items", "acme").await;
    post(&router, "/items", "globex").await;

    let (status, _) = post(&router, "/items/fail", "acme").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, acme) = get(&router, "/items", "acme").await;
    assert_eq!(acme, "acme", "the failed insert must not have committed");
    let (_, globex) = get(&router, "/items", "globex").await;
    assert_eq!(globex, "globex");

    directory.cleanup();
}

/// A transaction knows which tenant it ran for, and it agrees with the
/// extractor on the same request.
#[tokio::test]
async fn the_transaction_reports_the_tenant_it_ran_for() {
    let directory = Directory::provision();
    let (router, _pools) = app(&directory).await;

    let (status, body) = get(&router, "/memo", "globex").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "globex/globex");

    directory.cleanup();
}

/// A tenant the directory does not know is a 404 (not a 503), and nothing is
/// left behind for it.
#[tokio::test]
async fn an_unknown_tenant_is_a_404_and_leaves_no_pool() {
    let directory = Directory::provision();
    let (router, pools) = app(&directory).await;

    let (status, _) = post(&router, "/items", "ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        pools.active().is_empty(),
        "an unknown tenant must not leave a pool behind"
    );
    assert_eq!(directory.lookups(), 1);

    // The negative answer is cached: a second request does not re-query the
    // directory.
    let (status, _) = post(&router, "/items", "ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(directory.lookups(), 1);

    directory.cleanup();
}

/// A request that names no tenant is rejected before anything is opened.
#[tokio::test]
async fn a_request_without_a_tenant_never_reaches_the_directory() {
    let directory = Directory::provision();
    let (router, pools) = app(&directory).await;

    let (status, _) = send(&router, Method::POST, "/items", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(pools.active().is_empty());
    assert_eq!(directory.lookups(), 0);

    directory.cleanup();
}

/// Guards run before `#[managed]` acquisition: a rejected request costs no
/// connection, which is what keeps a hostile caller from opening one pool per
/// made-up tenant.
#[tokio::test]
async fn a_rejected_guard_opens_no_pool() {
    let directory = Directory::provision();
    let (router, pools) = app(&directory).await;

    let (status, _) = get(&router, "/guarded", "acme").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(pools.active().is_empty());
    assert_eq!(directory.lookups(), 0);

    directory.cleanup();
}

/// The memo: a route that extracts the tenant *and* opens a transaction must
/// resolve the tenant exactly once.
#[tokio::test]
async fn the_transaction_reuses_the_memoized_tenant() {
    let directory = Directory::provision();
    let resolver = Alternating::new(&["acme", "globex"]);
    let (router, _pools) = app_with(&directory, resolver.clone()).await;

    let (status, body) = get(&router, "/memo", "acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme/acme");
    assert_eq!(
        resolver.calls(),
        1,
        "the managed transaction must read the request's resolve-once cell, not resolve again"
    );

    directory.cleanup();
}

/// Finding 1: a handler whose *only* tenancy is two `#[managed]` transactions.
/// Nothing extracts the tenant, so the memo has to be installed independently
/// of any extractor — otherwise each transaction resolves for itself and the
/// two land on different databases.
#[tokio::test]
async fn two_managed_transactions_share_one_resolution() {
    let directory = Directory::provision();

    // One row in `acme` only (written through an ordinary header-resolved app),
    // so a `globex` transaction is visibly a *different database* and not just a
    // different label.
    let (seed, _) = app(&directory).await;
    assert_eq!(post(&seed, "/items", "acme").await.0, StatusCode::OK);

    let resolver = Alternating::new(&["acme", "globex"]);
    let (router, pools) = app_with(&directory, resolver.clone()).await;

    let (status, body) = get(&router, "/twin", "acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme:1/acme:1");
    assert_eq!(
        resolver.calls(),
        1,
        "both transactions must come from a single resolver call"
    );
    let active: Vec<String> = pools
        .active()
        .iter()
        .map(|tenant| tenant.as_str().to_string())
        .collect();
    assert_eq!(
        active,
        ["acme"],
        "a second resolution would have opened a second tenant's pool"
    );

    directory.cleanup();
}

/// Finding 1, the guard-first order: the guard resolves before `#[managed]`
/// acquisition, and the transaction must inherit that answer.
#[tokio::test]
async fn a_guard_that_resolves_first_settles_the_tenant() {
    let directory = Directory::provision();
    let resolver = Alternating::new(&["acme", "globex"]);
    let (router, _pools) = app_with(&directory, resolver.clone()).await;

    let (status, body) = get(&router, "/guard-first", "acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme");
    assert_eq!(
        resolver.calls(),
        1,
        "the guard and the transaction must share one resolution"
    );

    directory.cleanup();
}

/// Drive a request with a fake bearer token instead of a tenant header.
async fn get_as(router: &Router, path: &str, token: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// The tenant as a JWT claim, with the identity on the controller struct.
#[tokio::test]
async fn a_struct_identity_can_supply_the_tenant_claim() {
    let directory = Directory::provision();
    let (router, _pools) = app_with(&directory, ClaimResolver).await;

    let (status, body) = get_as(&router, "/jwt-struct", "alice@globex").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "alice@globex");

    directory.cleanup();
}

/// Finding 4: the same claim, with the identity on the *handler parameter*.
/// The generated closure has to extract it before it snapshots the request head
/// the `#[managed]` transaction resolves from — otherwise the resolver sees an
/// extensions map that authentication has not touched yet and the request fails
/// as tenant-less.
#[tokio::test]
async fn a_parameter_identity_can_supply_the_tenant_claim() {
    let directory = Directory::provision();
    let (router, _pools) = app_with(&directory, ClaimResolver).await;

    let (status, body) = get_as(&router, "/jwt-param", "bob@acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "bob@acme");

    directory.cleanup();
}
