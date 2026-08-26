//! The two plugins: [`Tenancy`] (install tenancy once) and [`PerTenant`]
//! (per-tenant resource of one type).
//!
//! ```ignore
//! AppBuilder::new()
//!     .load_config()?
//!     .provide(HeaderTenantResolver::default())
//!     .provide(TenantPools::new(directory))
//!     .plugin(Tenancy::resolver::<HeaderTenantResolver>())
//!     .plugin(PerTenant::<Pool<Postgres>>::from::<TenantPools>()
//!         .max_active(200)
//!         .idle_ttl(Duration::from_secs(300)))
//!     .build_state()
//!     .await
//! ```
//!
//! Both are `Plugin`s whose `build` runs inside `build_state()` as a
//! graph node: the resolver / source bean is resolved first (a declared
//! `Deps`), the typed `tenancy.*` config is guaranteed loaded, and the
//! provided bean ([`TenantRouter`], [`Tenanted<T>`]) is built whole — no shell,
//! no late fill. The resolver and the source stay ordinary beans with their own
//! `#[inject]` dependencies.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};

use crate::config::TenancyConfig;
use crate::error::TenantStatuses;
use crate::map::{Tenanted, TenantedSettings};
use crate::resolver::TenantResolver;
use crate::router::TenantRouter;
use crate::source::TenantSource;
use crate::TenantId;

/// Installs tenancy: provides the [`TenantRouter`] bean, backed by the resolver
/// bean `R`.
///
/// One per app. `R` is named as a type parameter and resolved from the graph
/// after `build_state()`, so the resolver can be a `#[bean]` with dependencies of
/// its own.
///
/// With `tenancy.enabled: false` the app still boots and still compiles — the
/// router is provided in a disabled mode where nothing resolves, so
/// `Option<Tenant<T>>` yields `None` and a required `Tenant<T>` reports a missing
/// tenant. Turning tenancy off does not require deleting code.
pub struct Tenancy<R = ()> {
    policy_override: Option<crate::config::MissingTenantPolicy>,
    statuses_override: Option<TenantStatuses>,
    _resolver: PhantomData<fn() -> R>,
}

impl Tenancy<()> {
    /// Install tenancy with `R` as the resolver bean.
    ///
    /// `R` must be provided (`.provide(..)`) or registered (`.register::<R>()`)
    /// on the same builder — a missing resolver is a `build_state()` error, since
    /// it is a declared plugin dependency.
    #[must_use]
    pub fn resolver<R>() -> Tenancy<R>
    where
        R: TenantResolver + Clone + Send + Sync + 'static,
    {
        Tenancy {
            policy_override: None,
            statuses_override: None,
            _resolver: PhantomData,
        }
    }
}

impl<R> Tenancy<R> {
    /// Reject requests that carry no tenant, overriding `tenancy.on-missing`.
    #[must_use]
    pub fn require_tenant(mut self) -> Self {
        self.policy_override = Some(crate::config::MissingTenantPolicy::Reject);
        self
    }

    /// Serve requests that carry no tenant (`Option` extractors see `None`),
    /// overriding `tenancy.on-missing`.
    #[must_use]
    pub fn allow_missing_tenant(mut self) -> Self {
        self.policy_override = Some(crate::config::MissingTenantPolicy::Allow);
        self
    }

    /// Override the three configurable statuses programmatically.
    #[must_use]
    pub fn statuses(mut self, statuses: TenantStatuses) -> Self {
        self.statuses_override = Some(statuses);
        self
    }
}

impl<R> Plugin for Tenancy<R>
where
    R: TenantResolver + Clone + Send + Sync + 'static,
{
    type Provided = (TenantRouter,);
    type Deps = (R,);
    type Config = TenancyConfig;
    type Controllers = ();

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    async fn build(
        self,
        (resolver,): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        // The resolve-once cell, installed before routing so that *every*
        // consumer of the request's tenant shares one resolver call — including
        // a handler whose only tenancy is a pair of `#[managed]` resources,
        // which never runs a tenancy extractor and only ever sees a read-only
        // `RequestHead`. Build effects are dropped when `tenancy.enabled` is
        // false, which is exactly right: a disabled router resolves nothing.
        ctx.add_layer(|router| {
            router.layer(r2e_core::http::middleware::from_fn(install_tenant_memo))
        });

        let mut config = config.unwrap_or_default();
        if let Some(policy) = self.policy_override {
            config.on_missing = Some(policy.as_str().to_string());
        }
        if let Some(statuses) = self.statuses_override {
            config.missing_status = Some(u64::from(statuses.missing.as_u16()));
            config.unknown_status = Some(u64::from(statuses.unknown.as_u16()));
            config.unavailable_status = Some(u64::from(statuses.unavailable.as_u16()));
        }

        // `tenancy.enabled = false` still provides the bean — the app keeps
        // compiling and booting; nothing resolves.
        if !ctx.enabled() {
            return Ok((TenantRouter::disabled(config.statuses()),));
        }
        Ok((TenantRouter::from_config(Arc::new(resolver), &config),))
    }
}

/// Park the per-request resolve-once cell in the request extensions.
///
/// One `Arc` allocation per request for apps that installed tenancy — the price
/// of a resolver that runs at most once per request on the success path,
/// whatever the shape of the route. A resolver **error** is not memoized: it is
/// returned to that caller and retried by the next resolution attempt in the
/// same request.
async fn install_tenant_memo(
    mut request: r2e_core::http::extract::Request,
    next: r2e_core::http::middleware::Next,
) -> r2e_core::http::response::Response {
    TenantRouter::install_memo(request.extensions_mut());
    next.run(request).await
}

/// Marker: no app-scoped fallback (the default).
pub struct NoFallback;

/// Marker: fall back to the app-scoped `T` bean when a resolved tenant is
/// unknown (`TenantSource::create` returns `Ok(None)`).
///
/// A request with no tenant never reaches the per-tenant map: `TenantRouter`
/// rejects it under the required policy, or an optional extractor yields
/// `None` under the allow policy.
pub struct DefaultFallback;

/// Installs one per-tenant resource type: provides the [`Tenanted<T>`] bean,
/// backed by the [`TenantSource<T>`] bean `Src`.
///
/// One per resource type. The type parameters are a small state machine:
/// `PerTenant<T>` is the un-sourced start, `PerTenant::<T>::from::<Src>()` names
/// the source, and [`fallback_to_default`](PerTenant::fallback_to_default)
/// switches to the variant that also requires `T` itself as a bean.
///
/// ```ignore
/// // strict: an unknown tenant is a 404
/// .plugin(PerTenant::<Pool<Postgres>>::from::<TenantPools>())
///
/// // fallback: an unknown tenant gets the app-scoped default pool
/// .provide(shared_pool.clone())
/// .plugin(PerTenant::<Pool<Postgres>>::from::<TenantPools>().fallback_to_default())
/// ```
pub struct PerTenant<T, Src = (), F = NoFallback> {
    max_active: Option<usize>,
    idle_ttl: Option<Option<Duration>>,
    create_timeout: Option<Option<Duration>>,
    negative_ttl: Option<Option<Duration>>,
    eager: Vec<TenantId>,
    _resource: PhantomData<fn() -> T>,
    _source: PhantomData<fn() -> Src>,
    _fallback: PhantomData<fn() -> F>,
}

impl<T> PerTenant<T, (), NoFallback>
where
    T: Clone + Send + Sync + 'static,
{
    /// Route `T` per tenant, built by the `Src` bean.
    ///
    /// `Src` must be provided or registered on the same builder.
    #[must_use]
    pub fn from<Src>() -> PerTenant<T, Src, NoFallback>
    where
        Src: TenantSource<T> + Clone + Send + Sync + 'static,
    {
        PerTenant::default_with_markers()
    }
}

impl<T, Src, F> PerTenant<T, Src, F> {
    fn default_with_markers() -> Self {
        Self {
            max_active: None,
            idle_ttl: None,
            create_timeout: None,
            negative_ttl: None,
            eager: Vec::new(),
            _resource: PhantomData,
            _source: PhantomData,
            _fallback: PhantomData,
        }
    }

    fn carry_over<F2>(self) -> PerTenant<T, Src, F2> {
        PerTenant {
            max_active: self.max_active,
            idle_ttl: self.idle_ttl,
            create_timeout: self.create_timeout,
            negative_ttl: self.negative_ttl,
            eager: self.eager,
            _resource: PhantomData,
            _source: PhantomData,
            _fallback: PhantomData,
        }
    }

    /// Cap live per-tenant resources, overriding `tenancy.max-active`.
    ///
    /// The knob that keeps a per-tenant pool from becoming
    /// `tenants × pool_size` connections: past the cap, the least recently used
    /// resources are evicted (and disposed).
    ///
    /// A **soft** cap: creation is not admission-controlled, so a cold burst can
    /// briefly exceed it and a background trim brings the map back down. Sizing a
    /// database's connection limit as `max_connections × max_active` is therefore
    /// not a hard guarantee.
    ///
    /// # Panics
    ///
    /// Panics on `max_active(0)` — a cap of zero creates every resource and
    /// evicts it straight away. Turn tenancy off with `tenancy.enabled: false`.
    #[must_use]
    pub fn max_active(mut self, max: usize) -> Self {
        assert!(
            max > 0,
            "`PerTenant::max_active(0)` is not a way to disable per-tenant resources: \
             pass at least 1 (or set `tenancy.enabled: false`)"
        );
        self.max_active = Some(max);
        self
    }

    /// Evict a resource unused for this long, overriding `tenancy.idle-ttl`.
    #[must_use]
    pub fn idle_ttl(mut self, ttl: Duration) -> Self {
        self.idle_ttl = Some(Some(ttl));
        self
    }

    /// Never evict for idleness.
    #[must_use]
    pub fn keep_forever(mut self) -> Self {
        self.idle_ttl = Some(None);
        self
    }

    /// Budget for one `create` call, overriding `tenancy.create-timeout`.
    #[must_use]
    pub fn create_timeout(mut self, timeout: Duration) -> Self {
        self.create_timeout = Some(Some(timeout));
        self
    }

    /// How long an unknown tenant is remembered, overriding
    /// `tenancy.negative-ttl`. Zero disables negative caching.
    #[must_use]
    pub fn negative_ttl(mut self, ttl: Duration) -> Self {
        self.negative_ttl = Some((!ttl.is_zero()).then_some(ttl));
        self
    }

    /// Create these tenants' resources at startup instead of on first request.
    ///
    /// For the handful of tenants that must not pay a cold start. Failures are
    /// logged, never fatal: a tenant whose database is down at boot must not stop
    /// the app from serving the others.
    #[must_use]
    pub fn eager<I>(mut self, tenants: I) -> Self
    where
        I: IntoIterator<Item = TenantId>,
    {
        self.eager.extend(tenants);
        self
    }

    /// Serve the app-scoped `T` bean when the source does not know a resolved
    /// tenant (`TenantSource::create` returns `Ok(None)`).
    ///
    /// The migration shape: an app that already has one shared `T` can adopt
    /// tenancy tenant-by-tenant, with every known request whose tenant is not
    /// yet migrated landing on the old shared resource. A request with no
    /// tenant is handled by `TenantRouter` before this map is called: it is
    /// rejected under the required policy or yields `None` from an optional
    /// extractor under the allow policy. This call adds `T` to the plugin's
    /// dependencies — the default must be a real bean. The fallback is
    /// app-scoped, so it is never cached per tenant and never disposed by
    /// eviction.
    #[must_use]
    pub fn fallback_to_default(self) -> PerTenant<T, Src, DefaultFallback> {
        self.carry_over()
    }

    fn settings(&self, config: Option<&TenancyConfig>) -> TenantedSettings {
        let mut settings = config.map_or_else(TenantedSettings::default, |c| {
            TenantedSettings::from_config(c)
        });
        if let Some(max) = self.max_active {
            settings.max_active = max;
        }
        if let Some(ttl) = self.idle_ttl {
            settings.idle_ttl = ttl;
        }
        if let Some(timeout) = self.create_timeout {
            settings.create_timeout = timeout;
        }
        if let Some(ttl) = self.negative_ttl {
            settings.negative_ttl = ttl;
        }
        settings
    }
}

impl<T, Src, F> PerTenant<T, Src, F>
where
    T: Clone + Send + Sync + 'static,
{
    /// Shared tail of both `build` impls: construct the map, start the sweeper,
    /// hook shutdown, warm up the eager tenants.
    ///
    /// The sweeper and the warm-up are surface effects, dropped when
    /// `tenancy.enabled` is false — a disabled router routes nothing into the
    /// map anyway, and the map itself is inert until something asks it for a
    /// tenant. The shutdown drain is *not* a surface effect and survives the
    /// gate: it disposes of what this build constructed, whatever the flag says.
    ///
    /// All three act on the `Tenanted<T>` the **graph** exposes, resolved when
    /// they run, not on the map this function returns — the two differ as soon
    /// as a test pins its own map (see the comment inside).
    fn build_map(
        self,
        source: Arc<dyn TenantSource<T>>,
        fallback: Option<T>,
        config: Option<TenancyConfig>,
        ctx: &mut PluginBuildContext,
    ) -> Tenanted<T> {
        let settings = self.settings(config.as_ref());
        let map = Tenanted::new(source, ctx.graph(), settings, fallback);

        // ── Resolve, don't capture ──────────────────────────────────────────
        // A test (or a migration step) may pin its own map with
        // `.override_bean(Tenanted::<T>::…)`. The projection out of this
        // plugin's build is then skipped while the group still builds, so
        // requests observe the PINNED map — and a sweeper, preload or drain
        // closed over `map` would groom an instance nothing can reach: the
        // served map would never be swept and never be drained, silently.
        // Every effect below therefore asks the graph for `Tenanted<T>` at the
        // moment it runs, exactly like the Scheduler's registry/token.
        //
        // The fallback to the map this build made is defensive, not a path we
        // know of: it covers a graph MISS — the plugin built a map but the
        // graph exposes no `Tenanted<T>` (a future projection-less install
        // shape, or a graph that never materialized). It is explicitly NOT
        // about `with_state`: that path never runs a plugin `build` at all,
        // so `build_map` is not reached. Grooming the built map on a miss is
        // strictly better than grooming nothing.

        // The sweeper: one task per map, cancelled by the app's shutdown token,
        // and awaited at shutdown so a drain actually closes what it opened.
        // Registered through `after_build` (still a surface effect, still
        // dropped when `tenancy.enabled` is false) so the graph is available.
        let sweeper_fallback = map.clone();
        let warming_fallback = map.clone();
        let eager = self.eager;
        ctx.after_build(move |dctx| {
            let sweeper = dctx
                .bean_context()
                .try_get::<Tenanted<T>>()
                .unwrap_or(sweeper_fallback);
            dctx.on_serve(move |sctx| {
                let token = sctx.shutdown_token();
                sctx.track(async move {
                    r2e_core::runtime::service::ServiceComponent::start(sweeper, token).await;
                });
            });

            if !eager.is_empty() {
                let warming = dctx
                    .bean_context()
                    .try_get::<Tenanted<T>>()
                    .unwrap_or(warming_fallback);
                dctx.on_serve(move |sctx| {
                    sctx.track(async move {
                        for (tenant, err) in warming.preload(eager).await {
                            tracing::warn!(
                                %tenant,
                                error = %err,
                                resource = std::any::type_name::<T>(),
                                "eager per-tenant resource creation failed; it will be retried on first request"
                            );
                        }
                    });
                });
            }
        });

        // Belt and braces: the sweeper drains on cancellation, but an app that
        // never serves (tests, `build_with_consumers`) still gets its resources
        // released when the shutdown hooks run. Cleanup is not a surface
        // effect, so it stays on `on_shutdown_async` (it survives the enabled
        // gate) and reaches the graph through the weak `GraphHandle` — alive
        // at that point, since shutdown hooks run inside the serving scope.
        let drain_fallback = map.clone();
        let drain_graph = ctx.graph();
        ctx.on_shutdown_async(move || async move {
            drain_graph
                .bean::<Tenanted<T>>()
                .unwrap_or(drain_fallback)
                .drain()
                .await;
        });

        map
    }
}

impl<T, Src> Plugin for PerTenant<T, Src, NoFallback>
where
    T: Clone + Send + Sync + 'static,
    Src: TenantSource<T> + Clone + Send + Sync + 'static,
{
    type Provided = (Tenanted<T>,);
    type Deps = (Src,);
    type Config = TenancyConfig;
    type Controllers = ();

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    async fn build(
        self,
        (source,): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((self.build_map(Arc::new(source), None, config, ctx),))
    }
}

impl<T, Src> Plugin for PerTenant<T, Src, DefaultFallback>
where
    T: Clone + Send + Sync + 'static,
    Src: TenantSource<T> + Clone + Send + Sync + 'static,
{
    type Provided = (Tenanted<T>,);
    /// `T` is a dependency here: the fallback default must be a real bean.
    type Deps = (Src, T);
    type Config = TenancyConfig;
    type Controllers = ();

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    async fn build(
        self,
        (source, default): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((self.build_map(Arc::new(source), Some(default), config, ctx),))
    }
}
