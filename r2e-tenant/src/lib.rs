//! Multi-tenant bean routing for R2E.
//!
//! One app process, many tenants, each with its **own** database pool, API
//! client, cache namespace or feature set — resolved from the request, created on
//! first use, reclaimed when idle. The three moving parts:
//!
//! | Piece | What it is | Who writes it |
//! |---|---|---|
//! | [`TenantResolver`] | request → [`TenantId`] | you (or a built-in like [`HeaderTenantResolver`]) |
//! | [`TenantSource<T>`] | tenant → `T` | you |
//! | [`Tenanted<T>`] | the map holding every tenant's `T` | the framework |
//!
//! Wiring is two plugins, and using it is one request-scoped field:
//!
//! ```ignore
//! use r2e::prelude::*;
//! use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy, Tenant};
//!
//! // 1. how a request names its tenant
//! .provide(HeaderTenantResolver::default())          // x-tenant-id
//! .plugin(Tenancy::resolver::<HeaderTenantResolver>())
//!
//! // 2. how a tenant gets its pool
//! .provide(TenantPools::new(directory))              // impl TenantSource<PgPool>
//! .plugin(PerTenant::<PgPool>::from::<TenantPools>().max_active(200))
//! ```
//!
//! ```ignore
//! #[controller(path = "/orders")]
//! struct OrderController {
//!     #[inject(request)]
//!     db: Tenant<PgPool>,   // this request's tenant's pool
//! }
//!
//! #[routes]
//! impl OrderController {
//!     #[get("/")]
//!     async fn list(&self) -> Result<Json<Vec<Order>>, HttpError> {
//!         Ok(Json(
//!             sqlx::query_as("select * from orders")
//!                 .fetch_all(&*self.db)
//!                 .await?,
//!         ))
//!     }
//! }
//! ```
//!
//! # What this buys over "just pass the tenant around"
//!
//! - **Forgetting the wiring is a compile error.** The extractors demand the
//!   [`TenantRouter`] and [`Tenanted<T>`] beans out of the HList state, so a
//!   missing `.plugin(..)` fails at `register_controllers`, not at the first
//!   request from the first tenant in production.
//! - **The resource lifecycle is handled.** Concurrent first requests for a
//!   tenant share one creation ([`Tenanted<T>`] single-flights it), failures are
//!   not cached, creation is bounded by a timeout, idle resources are evicted and
//!   disposed, and shutdown drains them.
//! - **Per-tenant resources compose.** A source receives a [`TenantContext`], so
//!   a per-tenant API client can be built on the *same* tenant's per-tenant pool
//!   ([`TenantContext::get`]) — with cycle detection.
//!
//! # Where the tenant comes from
//!
//! A resolver is a bean returning `Ok(None)` for "no tenant here"; what happens
//! then is the deployment's call ([`MissingTenantPolicy`], `tenancy.on-missing`),
//! not the resolver's. Built-ins cover headers, path parameters, request
//! extensions and closures; subdomains and JWT claims are a few lines each — see
//! the [`resolver`] module docs. A resolve-once cell, parked in the request by
//! the [`Tenancy`] layer before routing, is shared by every consumer — so
//! extractors, guards and `#[managed]` resources of one request see at most one
//! *successful* resolver call and one answer. An error is not memoized: it goes
//! back to that caller, and the next resolution attempt in the request tries
//! again.
//!
//! # Configuration
//!
//! Everything lives under `tenancy.*` ([`TenancyConfig`]); per-resource
//! overrides go on the [`PerTenant`] builder.
//!
//! ```yaml
//! tenancy:
//!   enabled: true          # false → the app boots, nothing resolves
//!   on-missing: reject     # or `allow` (Option extractors see None)
//!   max-active: 500        # live per-tenant resources, LRU beyond
//!   idle-ttl: 15m          # evict + dispose after this much idleness
//!   create-timeout: 10s    # per `create` call; blowing it is a 504
//!   negative-ttl: 5s       # how long an unknown tenant is remembered
//! ```
//!
//! # Failure mapping
//!
//! [`TenantError`] has one status per failure mode (400 missing, 404 unknown, 503
//! unavailable, 504 timeout, 500 for wiring bugs); the three request-driven ones
//! are configurable. See the [`error`] module docs.

#![deny(missing_docs)]

pub mod config;
pub mod error;
pub mod extract;
pub mod id;
pub mod map;
pub mod plugin;
pub mod resolver;
pub mod router;
pub mod source;

pub use config::{
    MissingTenantPolicy, TenancyConfig, DEFAULT_CREATE_TIMEOUT, DEFAULT_IDLE_TTL,
    DEFAULT_MAX_ACTIVE, DEFAULT_MAX_NEGATIVE, DEFAULT_NEGATIVE_TTL,
};
pub use error::{BoxError, TenantError, TenantStatuses};
pub use extract::Tenant;
pub use id::{InvalidTenantId, TenantId, MAX_TENANT_ID_LEN};
pub use map::{SweepReport, TenantStats, Tenanted, TenantedMetrics, TenantedSettings};
pub use plugin::{DefaultFallback, NoFallback, PerTenant, Tenancy};
pub use resolver::{
    BoxFuture, ExtensionTenantResolver, FnTenantResolver, HeaderTenantResolver, Lenient,
    PathTenantResolver, Strict, SyncTenantResolver, TenantResolver,
};
pub use router::TenantRouter;
pub use source::{TenantContext, TenantSource};
