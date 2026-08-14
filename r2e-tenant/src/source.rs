//! SPI #2 — building one tenant's copy of a resource, and the cascade.
//!
//! A [`TenantSource<T>`] is a **bean** that knows how to make a `T` for a
//! tenant: open a pool from the tenant's DSN, build an API client from the
//! tenant's credentials, derive a cache namespace. It is called at most once per
//! `(tenant, T)` — [`Tenanted<T>`](crate::Tenanted) single-flights it — and the
//! three answers it can give are all meaningful:
//!
//! | Return | Meaning | Result |
//! |---|---|---|
//! | `Ok(Some(value))` | provisioned | cached and served |
//! | `Ok(None)` | this tenant does not exist | 404 + negative cache (or the fallback bean) |
//! | `Err(cause)` | it exists but could not be built | 503, **not** cached — the next request retries |
//!
//! # The cascade
//!
//! `create` receives a [`TenantContext`], and that is what makes per-tenant
//! resources composable: `ctx.get::<U>()` resolves **`U` for the same tenant**,
//! recursing through `U`'s own source, so a per-tenant API client can be built
//! on top of the same tenant's per-tenant pool without either source knowing how
//! the other is wired.
//!
//! ```
//! use r2e_tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantSource};
//!
//! # #[derive(Clone)] struct Pool;
//! #[derive(Clone)]
//! struct ApiClient { pool: Pool, token: String }
//!
//! #[derive(Clone)]
//! struct ApiClients;
//!
//! impl TenantSource<ApiClient> for ApiClients {
//!     fn create<'a>(
//!         &'a self,
//!         tenant: &'a TenantId,
//!         ctx: &'a TenantContext<'a>,
//!     ) -> BoxFuture<'a, Result<Option<ApiClient>, BoxError>> {
//!         Box::pin(async move {
//!             // the SAME tenant's pool — created first if this is its first use
//!             let pool = ctx.get::<Pool>().await?;
//!             Ok(Some(ApiClient { pool, token: format!("{tenant}-token") }))
//!         })
//!     }
//! }
//! ```
//!
//! Cycles (`A` needs `B` needs `A`) are detected at runtime — per-tenant graphs
//! are built from live data, so there is nothing to check at compile time — and
//! reported as [`TenantError::Cycle`] naming the chain.

use std::any::{type_name, TypeId};
use std::sync::Arc;

use r2e_core::BeanContext;

use crate::error::{BoxError, TenantError};
use crate::map::Tenanted;
use crate::resolver::BoxFuture;
use crate::TenantId;

/// SPI #2: create (and dispose of) one tenant's copy of `T`.
pub trait TenantSource<T>: Send + Sync + 'static
where
    T: Clone + Send + Sync + 'static,
{
    /// Build `T` for `tenant`. `Ok(None)` means the tenant does not exist.
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<T>, BoxError>>;

    /// Release a tenant's resource: close the pool, flush the client.
    ///
    /// Called on idle/LRU eviction, on [`Tenanted::evict`], and on
    /// [`Tenanted::drain`] at shutdown — never on the app-scoped fallback bean.
    /// The default is a no-op, which is right for values whose `Drop` already
    /// releases everything.
    ///
    /// **Called at most once per cached value.** Each slot carries a one-shot
    /// gate, so an eviction racing a drain (or two sweeps) still disposes once;
    /// an implementation does not have to be idempotent. The gate is taken
    /// before the call, so a `dispose` that panics or is cancelled mid-await is
    /// **not** retried — at-most-once is the guarantee, not exactly-once.
    ///
    /// `value` is a clone of the cached resource: per-tenant resources are
    /// handle types (a pool, a client), and disposing a handle is how the shared
    /// object behind it gets closed. A request that resolved the resource just
    /// before it was evicted still holds its own clone — there is no lease — so
    /// disposal should be a *graceful* close (`sqlx`'s `Pool::close()` lets
    /// checked-out connections finish) rather than an abrupt one.
    fn dispose<'a>(&'a self, tenant: &'a TenantId, value: T) -> BoxFuture<'a, ()> {
        let _ = (tenant, value);
        Box::pin(std::future::ready(()))
    }
}

/// What a [`TenantSource`] gets to look at while building a resource.
///
/// Two lookups, deliberately different:
/// - [`get`](Self::get) — the **same tenant's** other per-tenant resources (the
///   cascade; lazy, single-flighted, cycle-checked).
/// - [`bean`](Self::bean) — plain app-scoped beans out of the graph.
pub struct TenantContext<'a> {
    tenant: &'a TenantId,
    beans: Arc<BeanContext>,
    chain: ResolutionChain,
}

impl<'a> TenantContext<'a> {
    pub(crate) fn new(
        tenant: &'a TenantId,
        beans: Arc<BeanContext>,
        chain: ResolutionChain,
    ) -> Self {
        Self {
            tenant,
            beans,
            chain,
        }
    }

    /// The tenant being provisioned.
    #[must_use]
    pub fn tenant(&self) -> &TenantId {
        self.tenant
    }

    /// Resolve another per-tenant resource **for this same tenant**.
    ///
    /// Finds `Tenanted<U>` in the app graph and resolves `U` through its own
    /// source, creating it if this is its first use. Fails with
    /// [`TenantError::NoSource`] when no `PerTenant<U, _>` plugin is installed,
    /// and with [`TenantError::Cycle`] when this would re-enter a type already
    /// being created for this tenant.
    pub async fn get<U: Clone + Send + Sync + 'static>(&self) -> Result<U, TenantError> {
        if self.chain.contains(TypeId::of::<U>()) {
            return Err(TenantError::Cycle(self.chain.describe_with::<U>()));
        }
        let map = self
            .beans
            .try_get::<Tenanted<U>>()
            .ok_or(TenantError::NoSource(type_name::<U>()))?;
        map.resolve(self.tenant, self.chain.push::<U>()).await
    }

    /// Clone an app-scoped bean out of the graph, `None` when absent.
    ///
    /// The witness-free counterpart of `#[inject]` — a source is constructed
    /// before any request, so it reads collaborators dynamically, exactly like
    /// a [`ManagedResource`](r2e_core::ManagedResource).
    #[must_use]
    pub fn bean<U: Clone + Send + Sync + 'static>(&self) -> Option<U> {
        self.beans.try_get::<U>()
    }

    /// The resolution chain that led here, most recent last (`"A -> B"`).
    #[must_use]
    pub fn chain(&self) -> String {
        self.chain.describe()
    }
}

/// The in-flight `(TypeId, name)` path of one tenant's resolution.
///
/// Cloned per hop; a per-tenant graph is a handful of types deep, so the copy is
/// cheaper than the machinery needed to thread a borrow through boxed futures.
#[derive(Clone, Default)]
pub(crate) struct ResolutionChain(Vec<(TypeId, &'static str)>);

impl ResolutionChain {
    pub(crate) fn root<T: 'static>() -> Self {
        Self(vec![(TypeId::of::<T>(), type_name::<T>())])
    }

    pub(crate) fn contains(&self, tid: TypeId) -> bool {
        self.0.iter().any(|(id, _)| *id == tid)
    }

    pub(crate) fn push<T: 'static>(&self) -> Self {
        let mut next = self.0.clone();
        next.push((TypeId::of::<T>(), type_name::<T>()));
        Self(next)
    }

    fn describe(&self) -> String {
        self.0
            .iter()
            .map(|(_, name)| short_name(name))
            .collect::<Vec<_>>()
            .join(" -> ")
    }

    fn describe_with<T: 'static>(&self) -> String {
        let mut chain = self.describe();
        chain.push_str(" -> ");
        chain.push_str(short_name(type_name::<T>()));
        chain
    }
}

/// Strip module paths so a cycle reads `A -> B -> A`, not
/// `my_app::tenancy::A -> ...`. Generic arguments are kept — `Pool<Postgres>`
/// and `Pool<Sqlite>` are different resources.
fn short_name(name: &'static str) -> &'static str {
    match name.find('<') {
        // Only the head is path-stripped; `a::b::Pool<c::d::Db>` keeps its args.
        Some(open) => match name[..open].rfind("::") {
            Some(sep) => &name[sep + 2..],
            None => name,
        },
        None => match name.rfind("::") {
            Some(sep) => &name[sep + 2..],
            None => name,
        },
    }
}
