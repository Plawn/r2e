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
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Method, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, BeanContext, GuardContext, Identity};
use r2e_data_diesel::{PoolSource, TenantPools, TenantTx};
use r2e_tenant::{
    FnTenantResolver, HeaderTenantResolver, MissingTenantPolicy, TenantId, TenantResolver,
    TenantRouter, TenantStatuses, TenantedSettings,
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
        .max_connections(1)
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

/// The whole app: a resolver, a per-tenant pool map over the directory, and the
/// three controllers. The map is returned too, so tests can ask which tenants
/// actually got a pool.
async fn app_with(
    directory: &Directory,
    resolver: Arc<dyn TenantResolver>,
) -> (Router, TenantPools<SqliteConnection>) {
    let pools = TenantPools::<SqliteConnection>::new(
        Arc::new(directory.source()),
        Arc::new(BeanContext::empty()),
        TenantedSettings::default(),
        None,
    );
    let router = AppBuilder::new()
        .provide(TenantRouter::ready(
            resolver,
            MissingTenantPolicy::Reject,
            TenantStatuses::default(),
        ))
        .provide(pools.clone())
        .build_state()
        .await
        .register_controller::<ItemController>()
        .register_controller::<MemoController>()
        .register_controller::<GuardedController>()
        .build();
    (router, pools)
}

/// The common case: `x-tenant-id`, requests without a tenant rejected.
async fn app(directory: &Directory) -> (Router, TenantPools<SqliteConnection>) {
    app_with(directory, Arc::new(HeaderTenantResolver::default())).await
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
    let resolutions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&resolutions);
    let resolver = FnTenantResolver::new(move |head: &r2e_core::request_head::RequestHead<'_>| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(head
            .header("x-tenant-id")
            .and_then(|raw| TenantId::parse(raw).ok()))
    });
    let (router, _pools) = app_with(&directory, Arc::new(resolver)).await;

    let (status, body) = get(&router, "/memo", "acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme/acme");
    assert_eq!(
        resolutions.load(Ordering::SeqCst),
        1,
        "the managed transaction must read the extractor's memo, not resolve again"
    );

    directory.cleanup();
}
