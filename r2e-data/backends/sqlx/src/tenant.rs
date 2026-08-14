//! Per-tenant SQLx pools and transactions (feature `tenant`).
//!
//! This is the database half of [`r2e_tenant`]: the generic crate routes a
//! request to a tenant and holds one resource per tenant, and this module makes
//! that resource an SQLx [`Pool`] plus a `#[managed]` transaction on it.
//!
//! ```ignore
//! use r2e::prelude::*;
//! use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};
//! use r2e_data_sqlx::{PoolSource, TenantTx};
//! use sqlx::{Pool, Postgres};
//!
//! // 1. how a request names its tenant
//! .provide(HeaderTenantResolver::default())               // x-tenant-id
//! .plugin(Tenancy::resolver::<HeaderTenantResolver>())
//! // 2. how a tenant gets its pool: slug -> DSN (a master-DB query here)
//! .provide(PoolSource::<Postgres>::new(move |tenant| {
//!     let directory = directory.clone();
//!     async move { directory.dsn_for(&tenant).await }
//! }))
//! .plugin(PerTenant::<Pool<Postgres>>::from::<PoolSource<Postgres>>().max_active(50))
//! ```
//!
//! ```ignore
//! #[post("/orders")]
//! async fn create(&self, #[managed] tx: &mut TenantTx<'_, Postgres>) -> Result<StatusCode, HttpError> {
//!     sqlx::query("INSERT INTO orders(name) VALUES ($1)")
//!         .bind("Ada")
//!         .execute(tx.connection())
//!         .await
//!         .map_err(|e| HttpError::internal(e.to_string()))?;
//!     Ok(StatusCode::CREATED)   // committed on the tenant's own database
//! }
//! ```
//!
//! # What the route has to declare
//!
//! Nothing beyond the `#[managed]` parameter. [`TenantPool`] lists the
//! [`TenantRouter`] and the [`TenantPools<DB>`] map in its
//! [`TxSource::Deps`](crate::TxSource::Deps), which `#[routes]` folds into the
//! controller's dependency list through
//! [`ManagedDeps`](r2e_core::ManagedDeps) — so a missing
//! `.plugin(Tenancy::resolver::<_>())` or `.plugin(PerTenant::<Pool<DB>>::from::<_>())`
//! is a **compile error** at `register_controller`, not a 500 on the first
//! request from the first tenant. A route may, but need not, also carry a
//! `#[inject(request)] tenant: TenantId` / `Tenant<Pool<DB>>` field: when it
//! does, the tenant resolved by the extractor is memoized in the request
//! extensions and the transaction reuses it instead of resolving again.
//!
//! # Rotating a tenant's DSN
//!
//! There is no `DbPool`-style rotation wrapper here: the master record *is* the
//! source of truth, so a changed DSN is
//! `pools.invalidate(&tenant)` ([`Tenanted::invalidate`]) — the next request
//! rebuilds the pool from the new record while the old one closes behind it.
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

use r2e_core::type_list::{TCons, TNil};
use r2e_core::{BeanLookup, HttpError, ManagedContext, ManagedErr};
use r2e_tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantRouter, TenantSource};
use sqlx::pool::PoolOptions;
use sqlx::{Database, Pool, Transaction};

use crate::tx::{ManagedTx, TxSource};

/// Every tenant's [`Pool<DB>`], keyed by tenant — the bean the
/// [`PerTenant`](r2e_tenant::PerTenant) plugin provides.
///
/// A plain [`Pool`], not a rotating [`DbPool`](crate::DbPool): a dynamic tenant
/// has no config key to watch, and its DSN changes through the tenant directory
/// (see [`Tenanted::invalidate`](r2e_tenant::Tenanted::invalidate)).
pub type TenantPools<DB> = r2e_tenant::Tenanted<Pool<DB>>;

/// [`TxSource`] that begins on the requesting tenant's pool.
///
/// Marker type only — use it through the [`TenantTx`] alias.
pub struct TenantPool<DB>(PhantomData<fn() -> DB>);

/// Managed transaction on the requesting tenant's pool.
///
/// The per-tenant counterpart of [`Tx`](crate::Tx); its
/// [`tenant()`](ManagedTx::tenant) reports which tenant it ran for.
pub type TenantTx<'a, DB> = ManagedTx<'a, DB, TenantPool<DB>>;

impl<DB: Database> TxSource<DB> for TenantPool<DB> {
    /// The tenant this transaction was begun for.
    type Meta = TenantId;
    type Deps = TCons<TenantRouter, TCons<TenantPools<DB>, TNil>>;

    async fn begin<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Transaction<'static, DB>, TenantId), ManagedErr<HttpError>>
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
        let pools = context.state.bean::<TenantPools<DB>>().ok_or_else(|| {
            context.missing_bean(
                "per-tenant pool bean",
                type_name::<TenantPools<DB>>(),
                "call .plugin(PerTenant::<Pool<_>>::from::<MySource>())",
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
        let transaction = pool
            .begin()
            .await
            .map_err(|error| ManagedErr(HttpError::internal(error.to_string())))?;
        Ok((transaction, tenant))
    }
}

impl<'a, DB: Database> ManagedTx<'a, DB, TenantPool<DB>> {
    /// The tenant whose pool this transaction was begun on.
    #[must_use]
    pub fn tenant(&self) -> &TenantId {
        self.meta()
    }
}

/// A [`TenantSource`] that opens one [`Pool<DB>`] per tenant from a DSN lookup.
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
/// PoolSource::<Postgres>::new(move |tenant| {
///     let master = master.clone();
///     async move {
///         let dsn: Option<String> = sqlx::query_scalar("SELECT dsn FROM tenants WHERE slug = $1")
///             .bind(tenant.as_str())
///             .fetch_optional(&master)
///             .await?;
///         Ok(dsn)
///     }
/// })
/// .with_options(PgPoolOptions::new().max_connections(4))
///
/// // sync lookup (a static map, config, a warm cache)
/// PoolSource::<Sqlite>::sync(move |tenant| dsns.get(tenant.as_str()).cloned())
/// ```
///
/// `dispose` closes the pool explicitly (`Pool::close().await`), so an evicted
/// tenant's connections are released rather than left to the last handle's
/// `Drop`.
pub struct PoolSource<DB: Database> {
    lookup: Arc<dyn DsnLookup>,
    options: PoolOptions<DB>,
}

/// The boxed DSN lookup behind a [`PoolSource`].
///
/// Boxed rather than a type parameter on purpose: `PerTenant::<T>::from::<Src>()`
/// names the source **type**, and a closure type cannot be named — a generic
/// `PoolSource<DB, F>` would be unusable with the plugin.
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

impl<DB: Database> PoolSource<DB> {
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
            options: PoolOptions::new(),
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

    /// Use these [`PoolOptions`] for every tenant's pool.
    ///
    /// The knob that keeps per-tenant pooling affordable: `max_connections`
    /// here multiplies by the number of live tenants, which
    /// [`PerTenant::max_active`](r2e_tenant::PerTenant::max_active) caps.
    #[must_use]
    pub fn with_options(mut self, options: PoolOptions<DB>) -> Self {
        self.options = options;
        self
    }

    /// Cap each tenant's pool at `max` connections (shorthand for the matching
    /// [`with_options`](Self::with_options) call).
    #[must_use]
    pub fn max_connections(mut self, max: u32) -> Self {
        self.options = self.options.max_connections(max);
        self
    }
}

impl<DB: Database> Clone for PoolSource<DB> {
    fn clone(&self) -> Self {
        Self {
            lookup: Arc::clone(&self.lookup),
            options: self.options.clone(),
        }
    }
}

impl<DB: Database> std::fmt::Debug for PoolSource<DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSource")
            .field("database", &type_name::<DB>())
            .finish_non_exhaustive()
    }
}

impl<DB: Database> TenantSource<Pool<DB>> for PoolSource<DB> {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        _ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Pool<DB>>, BoxError>> {
        Box::pin(async move {
            let Some(dsn) = self.lookup.dsn(tenant.clone()).await? else {
                return Ok(None);
            };
            // `connect` opens the first connection, so a tenant whose database
            // is unreachable fails here (503) instead of on the first query —
            // and the whole call is bounded by `tenancy.create-timeout`.
            let pool = self.options.clone().connect(&dsn).await?;
            Ok(Some(pool))
        })
    }

    fn dispose<'a>(&'a self, _tenant: &'a TenantId, pool: Pool<DB>) -> BoxFuture<'a, ()> {
        Box::pin(async move { pool.close().await })
    }
}
