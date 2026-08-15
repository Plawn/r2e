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
//! Both are `PreStatePlugin`s that provide a [`Late`](r2e_core::Late)-backed
//! shell at install time and fill it in `configure`, once the graph holds the
//! resolver / source bean. That split is what lets the resolver and the source be
//! ordinary beans — with their own `#[inject]` dependencies — while the beans that
//! controllers demand (`TenantRouter`, `Tenanted<T>`) exist early enough to be in
//! the state type.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use r2e_core::plugin::{DeferredContext, PluginInstallContext, PreStatePlugin};

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

impl<R> PreStatePlugin for Tenancy<R>
where
    R: TenantResolver + Clone + Send + Sync + 'static,
{
    type Provided = (TenantRouter,);
    type Deps = (R,);
    type Config = TenancyConfig;

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        // The resolve-once cell, installed before routing so that *every*
        // consumer of the request's tenant shares one resolver call — including
        // a handler whose only tenancy is a pair of `#[managed]` resources,
        // which never runs a tenancy extractor and only ever sees a read-only
        // `RequestHead`. Sugar actions are skipped when `tenancy.enabled` is
        // false, which is exactly right: a disabled router resolves nothing.
        ctx.add_layer(|router| {
            router.layer(r2e_core::http::middleware::from_fn(install_tenant_memo))
        });

        // `tenancy.enabled = false` skips `configure` entirely, so the disabled
        // router has to be built here — an unwired router is a *wiring bug*
        // (500), which is not what turning tenancy off should mean. The app keeps
        // compiling and booting; nothing resolves.
        if ctx.config_get::<bool>("tenancy.enabled") == Some(false) {
            let statuses = self
                .statuses_override
                .unwrap_or_else(|| statuses_from_keys(ctx));
            return (TenantRouter::disabled(statuses),);
        }
        (TenantRouter::unwired(),)
    }

    fn configure(
        self,
        (router,): &Self::Provided,
        (resolver,): Self::Deps,
        config: Option<Self::Config>,
        _ctx: &mut DeferredContext<'_>,
    ) {
        let mut config = config.unwrap_or_default();
        if let Some(policy) = self.policy_override {
            config.on_missing = Some(policy.as_str().to_string());
        }
        if let Some(statuses) = self.statuses_override {
            config.missing_status = Some(u64::from(statuses.missing.as_u16()));
            config.unknown_status = Some(u64::from(statuses.unknown.as_u16()));
            config.unavailable_status = Some(u64::from(statuses.unavailable.as_u16()));
        }
        router.wire(Arc::new(resolver), &config);
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

/// The configured statuses, read key by key — the typed section is not
/// available at install time (it is loaded for `configure`).
fn statuses_from_keys(ctx: &PluginInstallContext<'_>) -> TenantStatuses {
    let config = TenancyConfig {
        missing_status: ctx.config_get::<u64>("tenancy.missing-status"),
        unknown_status: ctx.config_get::<u64>("tenancy.unknown-status"),
        unavailable_status: ctx.config_get::<u64>("tenancy.unavailable-status"),
        ..TenancyConfig::default()
    };
    config.statuses()
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
    /// Shared tail of both `configure` impls: fill the map, start the sweeper,
    /// hook shutdown, warm up the eager tenants.
    fn wire(
        self,
        map: &Tenanted<T>,
        source: Arc<dyn TenantSource<T>>,
        fallback: Option<T>,
        config: Option<TenancyConfig>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let settings = self.settings(config.as_ref());
        map.wire(source, ctx.bean_context_handle(), settings, fallback);

        // The sweeper: one task per map, cancelled by the app's shutdown token,
        // and awaited at shutdown so a drain actually closes what it opened.
        let sweeper = map.clone();
        ctx.on_serve(move |sctx| {
            let token = sctx.shutdown_token();
            sctx.track(r2e_core::rt::spawn(async move {
                r2e_core::service::ServiceComponent::start(sweeper, token).await;
            }));
        });

        // Belt and braces: the sweeper drains on cancellation, but an app that
        // never serves (tests, `build_with_consumers`) still gets its resources
        // released when the shutdown hooks run.
        let draining = map.clone();
        ctx.on_shutdown_async(move || async move { draining.drain().await });

        if !self.eager.is_empty() {
            let warming = map.clone();
            let tenants = self.eager;
            ctx.on_serve(move |sctx| {
                sctx.track(r2e_core::rt::spawn(async move {
                    for (tenant, err) in warming.preload(tenants).await {
                        tracing::warn!(
                            %tenant,
                            error = %err,
                            resource = std::any::type_name::<T>(),
                            "eager per-tenant resource creation failed; it will be retried on first request"
                        );
                    }
                }));
            });
        }
    }
}

impl<T, Src> PreStatePlugin for PerTenant<T, Src, NoFallback>
where
    T: Clone + Send + Sync + 'static,
    Src: TenantSource<T> + Clone + Send + Sync + 'static,
{
    type Provided = (Tenanted<T>,);
    type Deps = (Src,);
    type Config = TenancyConfig;

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        (Tenanted::unwired(),)
    }

    fn configure(
        self,
        (map,): &Self::Provided,
        (source,): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut DeferredContext<'_>,
    ) {
        self.wire(map, Arc::new(source), None, config, ctx);
    }
}

impl<T, Src> PreStatePlugin for PerTenant<T, Src, DefaultFallback>
where
    T: Clone + Send + Sync + 'static,
    Src: TenantSource<T> + Clone + Send + Sync + 'static,
{
    type Provided = (Tenanted<T>,);
    /// `T` is a dependency here: the fallback default must be a real bean.
    type Deps = (Src, T);
    type Config = TenancyConfig;

    const CONFIG_PREFIX: Option<&'static str> = Some("tenancy");

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        (Tenanted::unwired(),)
    }

    fn configure(
        self,
        (map,): &Self::Provided,
        (source, default): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut DeferredContext<'_>,
    ) {
        self.wire(map, Arc::new(source), Some(default), config, ctx);
    }
}
