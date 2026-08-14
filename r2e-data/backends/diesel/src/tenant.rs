//! Per-tenant Diesel pools and transactions (feature `tenant`).
//!
//! This is the Diesel half of [`r2e_tenant`]: the generic crate routes a request
//! to a tenant and holds one resource per tenant, and this module makes that
//! resource an r2d2 [`Pool<ConnectionManager<Conn>>`] plus a `#[managed]`
//! transaction on it. It mirrors the SQLx backend's `tenant` module type for
//! type; the only differences are r2d2's shape (pools are built on a blocking
//! thread and are never explicitly closed) and Diesel's per-connection-type
//! generics — `Conn`, not `DB`.
//!
//! ```ignore
//! use r2e::prelude::*;
//! use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};
//! use r2e_data_diesel::{PoolSource, TenantTx};
//! use diesel::r2d2::{ConnectionManager, Pool};
//! use diesel::PgConnection;
//!
//! // 1. how a request names its tenant
//! .provide(HeaderTenantResolver::default())               // x-tenant-id
//! .plugin(Tenancy::resolver::<HeaderTenantResolver>())
//! // 2. how a tenant gets its pool: slug -> DSN (a master-DB query here)
//! .provide(PoolSource::<PgConnection>::new(move |tenant| {
//!     let directory = directory.clone();
//!     async move { directory.dsn_for(&tenant).await }
//! }).max_connections(4))
//! .plugin(PerTenant::<Pool<ConnectionManager<PgConnection>>>::from::<PoolSource<PgConnection>>()
//!     .max_active(50))
//! ```
//!
//! ```ignore
//! #[post("/orders")]
//! async fn create(&self, #[managed] tx: &mut TenantTx<PgConnection>) -> Result<StatusCode, HttpError> {
//!     tx.run(|connection| {
//!         diesel::insert_into(orders::table)
//!             .values(orders::name.eq("Ada"))
//!             .execute(connection)
//!     })
//!     .await?;
//!     Ok(StatusCode::CREATED)   // committed on the tenant's own database
//! }
//! ```
//!
//! # What the route has to declare
//!
//! Nothing beyond the `#[managed]` parameter. [`TenantPool`] lists the
//! [`TenantRouter`] and the [`TenantPools<Conn>`] map in its
//! [`TxSource::Deps`](crate::TxSource::Deps), which `#[routes]` folds into the
//! controller's dependency list through [`ManagedDeps`](r2e_core::ManagedDeps) —
//! so a missing `.plugin(Tenancy::resolver::<_>())` or
//! `.plugin(PerTenant::<Pool<ConnectionManager<Conn>>>::from::<_>())` is a
//! **compile error** at `register_controller`, not a 500 on the first request
//! from the first tenant. A route may, but need not, also carry a
//! `#[inject(request)] tenant: TenantId` / `Tenant<Pool<..>>` field: when it
//! does, the tenant resolved by the extractor is memoized in the request
//! extensions and the transaction reuses it instead of resolving again.
//!
//! # Rotating a tenant's DSN
//!
//! There is no `DbPool`-style rotation wrapper here: the master record *is* the
//! source of truth, so a changed DSN is `pools.invalidate(&tenant)`
//! ([`Tenanted::invalidate`](r2e_tenant::Tenanted::invalidate)) — the next
//! request builds a pool from the new record, and the old r2d2 pool goes away
//! once its last handle and connection do.
//!
//! # Deferred: per-tenant migrations
//!
//! Running migrations when a tenant's pool is first created is deliberately out
//! of scope. It belongs inside the single-flight cell (so N concurrent first
//! requests migrate once), which means it runs under `tenancy.create-timeout` —
//! a migration set that takes longer than that budget would surface as a 504 for
//! the tenant that triggered it. Until that interaction is designed, migrate
//! tenants from your provisioning path, not from the request path.

use std::any::type_name;
use std::marker::PhantomData;
use std::sync::Arc;

use diesel::r2d2::{ConnectionManager, Pool, R2D2Connection};
use diesel::Connection;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{BeanLookup, HttpError, ManagedContext, ManagedErr};
use r2e_tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantRouter, TenantSource};

use crate::pool::PoolError;
use crate::tx::{ManagedTx, TxSource};

/// Every tenant's r2d2 [`Pool`], keyed by tenant — the bean the
/// [`PerTenant`](r2e_tenant::PerTenant) plugin provides.
///
/// A plain [`Pool`], not a rotating [`DbPool`](crate::DbPool): a dynamic tenant
/// has no config key to watch, and its DSN changes through the tenant directory
/// (see [`Tenanted::invalidate`](r2e_tenant::Tenanted::invalidate)).
pub type TenantPools<Conn> = r2e_tenant::Tenanted<Pool<ConnectionManager<Conn>>>;

/// [`TxSource`] that checks a connection out of the requesting tenant's pool.
///
/// Marker type only — use it through the [`TenantTx`] alias.
pub struct TenantPool<Conn>(PhantomData<fn() -> Conn>);

/// Managed transaction on the requesting tenant's pool.
///
/// The per-tenant counterpart of [`Tx`](crate::Tx); its
/// [`tenant()`](ManagedTx::tenant) reports which tenant it ran for.
pub type TenantTx<Conn> = ManagedTx<Conn, TenantPool<Conn>>;

impl<Conn> TxSource<Conn> for TenantPool<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    /// The tenant this transaction was begun for.
    type Meta = TenantId;
    type Deps = TCons<TenantRouter, TCons<TenantPools<Conn>, TNil>>;

    async fn acquire_pool<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Pool<ConnectionManager<Conn>>, TenantId), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync,
    {
        let head = context.require_request()?;
        let router = context.state.bean::<TenantRouter>().ok_or_else(|| {
            context.missing_bean(
                "tenant router bean",
                type_name::<TenantRouter>(),
                "call .plugin(Tenancy::resolver::<MyResolver>())",
            )
        })?;
        let pools = context.state.bean::<TenantPools<Conn>>().ok_or_else(|| {
            context.missing_bean(
                "per-tenant pool bean",
                type_name::<TenantPools<Conn>>(),
                "call .plugin(PerTenant::<Pool<ConnectionManager<_>>>::from::<MySource>())",
            )
        })?;

        // The extractors park the resolved tenant in the request extensions, so
        // a route that also has a `Tenant<T>` / `TenantId` field resolves once.
        // Falling back to `resolve` rather than caching anything here keeps this
        // path stateless: header/path resolution is a map lookup, and a resolver
        // that is genuinely expensive memoizes through the extensions itself.
        let tenant = match TenantRouter::memoized(&head) {
            Some(memoized) => memoized.clone(),
            None => router.resolve(&head).await.map_err(ManagedErr)?,
        };

        let pool = pools
            .get(&tenant)
            .await
            .map_err(|error| ManagedErr(error.into_http_error(pools.statuses())))?;
        Ok((pool, tenant))
    }
}

impl<Conn> ManagedTx<Conn, TenantPool<Conn>>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// The tenant whose pool this transaction was begun on.
    #[must_use]
    pub fn tenant(&self) -> &TenantId {
        self.meta()
    }
}

/// Builds one tenant's r2d2 pool from its DSN. Cloned into a blocking task on
/// every creation, since r2d2's pool builder is blocking.
type PoolFactory<Conn> =
    Arc<dyn Fn(String) -> Result<Pool<ConnectionManager<Conn>>, PoolError> + Send + Sync>;

/// A [`TenantSource`] that opens one r2d2 [`Pool`] per tenant from a DSN lookup.
///
/// The lookup is the app's tenant directory — usually a query against a master
/// database — and its three answers are the three the SPI defines:
///
/// | Lookup returns | Meaning | What the caller sees |
/// |---|---|---|
/// | `Ok(Some(dsn))` | provisioned | the pool, cached until idle |
/// | `Ok(None)` | no such tenant | `tenancy.unknown-status` (404 by default), negatively cached |
/// | `Err(cause)` | the directory itself failed | `tenancy.unavailable-status` (503), **not** cached — the next request retries |
///
/// Returning `Err` for an unknown tenant would tell clients and load balancers
/// to retry something that will never work, so keep the distinction.
///
/// ```ignore
/// // async lookup (the normal case: a master-DB query)
/// PoolSource::<PgConnection>::new(move |tenant| {
///     let master = master.clone();
///     async move { Ok(master.dsn_for(tenant.as_str()).await?) }
/// })
/// .max_connections(4)
///
/// // sync lookup (a static map, config, a warm cache)
/// PoolSource::<SqliteConnection>::sync(move |tenant| dsns.get(tenant.as_str()).cloned())
///
/// // full control over the r2d2 builder
/// PoolSource::<PgConnection>::sync(lookup).with_factory(|dsn| {
///     Pool::builder()
///         .max_size(4)
///         .connection_timeout(Duration::from_secs(2))
///         .build(ConnectionManager::new(dsn))
///         .map_err(|error| PoolError(error.to_string()))
/// })
/// ```
///
/// There is no `dispose`: r2d2 pools have no close operation. An evicted
/// tenant's pool is released when the map drops its handle and the last
/// outstanding connection goes away — which is exactly what `Drop` already
/// guarantees, so the SPI's no-op default is the correct behaviour here (the
/// SQLx source, whose pools *can* be closed, overrides it).
pub struct PoolSource<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    lookup: Arc<dyn DsnLookup>,
    factory: PoolFactory<Conn>,
}

/// The boxed DSN lookup behind a [`PoolSource`].
///
/// Boxed rather than a type parameter on purpose: `PerTenant::<T>::from::<Src>()`
/// names the source **type**, and a closure type cannot be named — a generic
/// `PoolSource<Conn, F>` would be unusable with the plugin.
trait DsnLookup: Send + Sync + 'static {
    fn dsn(&self, tenant: TenantId) -> BoxFuture<'static, Result<Option<String>, BoxError>>;
}

impl<F, Fut> DsnLookup for F
where
    F: Fn(TenantId) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Option<String>, BoxError>> + Send + 'static,
{
    fn dsn(&self, tenant: TenantId) -> BoxFuture<'static, Result<Option<String>, BoxError>> {
        Box::pin(self(tenant))
    }
}

impl<Conn> PoolSource<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// A source whose DSN lookup is asynchronous — the normal case, since a
    /// tenant directory lives in a database or a remote service.
    #[must_use]
    pub fn new<F, Fut>(lookup: F) -> Self
    where
        F: Fn(TenantId) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Option<String>, BoxError>> + Send + 'static,
    {
        Self {
            lookup: Arc::new(lookup),
            factory: default_factory::<Conn>(),
        }
    }

    /// A source whose DSN lookup needs no `.await` — a static map, a config
    /// section, an already-warm cache.
    #[must_use]
    pub fn sync<F>(lookup: F) -> Self
    where
        F: Fn(&TenantId) -> Option<String> + Send + Sync + 'static,
    {
        Self::new(move |tenant: TenantId| {
            let dsn = lookup(&tenant);
            std::future::ready(Ok(dsn))
        })
    }

    /// Build every tenant's pool with this factory.
    ///
    /// The Diesel counterpart of SQLx's `with_options`: r2d2's `Builder` is not
    /// clonable, so the knob is a closure that produces a pool from a DSN — the
    /// same shape [`DbPool::connect_with`](crate::DbPool::connect_with) takes.
    /// It runs on a blocking thread.
    #[must_use]
    pub fn with_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(String) -> Result<Pool<ConnectionManager<Conn>>, PoolError> + Send + Sync + 'static,
    {
        self.factory = Arc::new(factory);
        self
    }

    /// Cap each tenant's pool at `max` connections.
    ///
    /// The knob that keeps per-tenant pooling affordable: `max` here multiplies
    /// by the number of live tenants, which
    /// [`PerTenant::max_active`](r2e_tenant::PerTenant::max_active) caps.
    /// Replaces any previously installed [`with_factory`](Self::with_factory).
    #[must_use]
    pub fn max_connections(self, max: u32) -> Self {
        self.with_factory(move |dsn| {
            Pool::builder()
                .max_size(max)
                .build(ConnectionManager::<Conn>::new(dsn))
                .map_err(|error| PoolError(error.to_string()))
        })
    }
}

impl<Conn> Clone for PoolSource<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    fn clone(&self) -> Self {
        Self {
            lookup: Arc::clone(&self.lookup),
            factory: Arc::clone(&self.factory),
        }
    }
}

impl<Conn> std::fmt::Debug for PoolSource<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSource")
            .field("connection", &type_name::<Conn>())
            .finish_non_exhaustive()
    }
}

impl<Conn> TenantSource<Pool<ConnectionManager<Conn>>> for PoolSource<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        _ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Pool<ConnectionManager<Conn>>>, BoxError>> {
        Box::pin(async move {
            let Some(dsn) = self.lookup.dsn(tenant.clone()).await? else {
                return Ok(None);
            };
            // r2d2 builds pools by opening connections, which blocks — and the
            // whole call is bounded by `tenancy.create-timeout`, so a tenant
            // whose database is unreachable surfaces as 503/504 here instead of
            // stalling the first query.
            let factory = Arc::clone(&self.factory);
            let pool = tokio::task::spawn_blocking(move || factory(dsn))
                .await
                .map_err(|error| {
                    Box::new(PoolError(format!("pool build task failed: {error}"))) as BoxError
                })??;
            Ok(Some(pool))
        })
    }
}

fn default_factory<Conn>() -> PoolFactory<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    Arc::new(|dsn| {
        Pool::builder()
            .build(ConnectionManager::<Conn>::new(dsn))
            .map_err(|error| PoolError(error.to_string()))
    })
}
