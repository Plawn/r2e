//! [`Tenanted<T>`] — one bean holding every tenant's copy of `T`.
//!
//! `Tenanted<T>` is an ordinary app-scoped bean (`Clone` = refcount bump) whose
//! job is to turn a [`TenantId`] into a `T`, creating it on first use and
//! keeping it alive while it is being used. One `TypeId` per resource type, so
//! the state can carry `Tenanted<Pool<Postgres>>` and `Tenanted<ApiClient>`
//! side by side and the extractors can demand exactly the one they need.
//!
//! # Invariants
//!
//! These are the properties the implementation is built to keep; they are what
//! the tests in `tests/tenant/map.rs` pin down.
//!
//! - **Single flight.** N concurrent requests for a cold tenant produce exactly
//!   one `create` call. The map holds `Arc<Slot<T>>` values and the `Arc` is
//!   cloned out of the `DashMap` *before* any `.await` — holding a shard guard
//!   across an await would deadlock the map.
//! - **Failures are never cached.** An `Err` from `create` leaves nothing
//!   behind: the empty slot is removed (guarded by `Arc::ptr_eq`, so a
//!   concurrent retry's slot is never stolen) and the next request tries again.
//!   That also means a flood of made-up tenant ids cannot accumulate slots.
//! - **Unknown tenants are cached, briefly.** `Ok(None)` is remembered for
//!   `negative-ttl` (capped at `max-negative` entries) so a hot 404 does not
//!   hammer the tenant directory; any later success clears the entry.
//! - **Creation is bounded.** `create` runs under `create-timeout`; blowing it is
//!   a `504`, and every waiter parked on the slot is released.
//! - **Idle resources go away.** A background sweep (the [`ServiceComponent`]
//!   impl, started by the [`PerTenant`](crate::PerTenant) plugin) evicts
//!   resources unused for `idle-ttl`, trims the map to `max-active` by least
//!   recent use, and disposes what it removes. Shutdown drains everything.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{BeanContext, Late};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::config::{TenancyConfig, DEFAULT_MAX_ACTIVE, DEFAULT_MAX_NEGATIVE};
use crate::error::{BoxError, TenantError, TenantStatuses};
use crate::source::{ResolutionChain, TenantContext, TenantSource};
use crate::TenantId;

/// Per-resource knobs: the `tenancy.*` defaults after the
/// [`PerTenant`](crate::PerTenant) builder overrides have been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantedSettings {
    /// Cap on live per-tenant resources; the excess is evicted least-recent-first.
    pub max_active: usize,
    /// Evict a resource unused for this long. `None` disables idle eviction.
    pub idle_ttl: Option<Duration>,
    /// Budget for one `create` call. `None` disables the timeout.
    pub create_timeout: Option<Duration>,
    /// How long an unknown tenant is remembered. `None` disables negative caching.
    pub negative_ttl: Option<Duration>,
    /// Cap on negative-cache entries.
    pub max_negative: usize,
    /// How tenancy failures map to HTTP statuses.
    pub statuses: TenantStatuses,
}

impl Default for TenantedSettings {
    fn default() -> Self {
        Self {
            max_active: DEFAULT_MAX_ACTIVE,
            idle_ttl: crate::config::TenancyConfig::default().idle_ttl(),
            create_timeout: crate::config::TenancyConfig::default().create_timeout(),
            negative_ttl: crate::config::TenancyConfig::default().negative_ttl(),
            max_negative: DEFAULT_MAX_NEGATIVE,
            statuses: TenantStatuses::default(),
        }
    }
}

impl TenantedSettings {
    /// The settings a `tenancy.*` section asks for, before per-resource
    /// overrides.
    #[must_use]
    pub fn from_config(config: &TenancyConfig) -> Self {
        Self {
            max_active: config.max_active(),
            idle_ttl: config.idle_ttl(),
            create_timeout: config.create_timeout(),
            negative_ttl: config.negative_ttl(),
            max_negative: config.max_negative(),
            statuses: config.statuses(),
        }
    }

    /// How often the background sweep runs: a quarter of `idle-ttl`, clamped to
    /// `[1s, 60s]` — often enough that eviction is timely, rarely enough that an
    /// idle app stays idle.
    #[must_use]
    pub fn sweep_interval(&self) -> Duration {
        let base = self.idle_ttl.unwrap_or(Duration::from_secs(120)) / 4;
        base.clamp(Duration::from_secs(1), Duration::from_secs(60))
    }
}

/// Every tenant's copy of `T`, created on demand.
pub struct Tenanted<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for Tenanted<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<T> {
    slots: DashMap<TenantId, Arc<Slot<T>>>,
    negative: DashMap<TenantId, u64>,
    wiring: Late<Wiring<T>>,
    epoch: Instant,
    trimming: AtomicBool,
    counters: Counters,
}

struct Slot<T> {
    cell: OnceCell<T>,
    last_used: AtomicU64,
}

struct Wiring<T> {
    source: Arc<dyn TenantSource<T>>,
    beans: Arc<BeanContext>,
    settings: TenantedSettings,
    /// The app-scoped default, when `fallback_to_default()` was asked for.
    fallback: Option<T>,
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    created: AtomicU64,
    create_failures: AtomicU64,
    timeouts: AtomicU64,
    unknown: AtomicU64,
    fallbacks: AtomicU64,
    disposed: AtomicU64,
    evicted_idle: AtomicU64,
    evicted_lru: AtomicU64,
}

/// A point-in-time view of one [`Tenanted<T>`] map.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TenantedMetrics {
    /// Tenants with a live resource right now.
    pub active: usize,
    /// Unknown tenants currently remembered.
    pub negative: usize,
    /// Resources served from cache.
    pub hits: u64,
    /// Resources created.
    pub created: u64,
    /// `create` calls that returned an error.
    pub create_failures: u64,
    /// `create` calls that hit `create-timeout`.
    pub timeouts: u64,
    /// `create` calls that reported an unknown tenant.
    pub unknown: u64,
    /// Requests served with the app-scoped fallback bean.
    pub fallbacks: u64,
    /// Resources handed to `dispose`.
    pub disposed: u64,
    /// Resources evicted for being idle.
    pub evicted_idle: u64,
    /// Resources evicted to stay under `max-active`.
    pub evicted_lru: u64,
}

/// Per-tenant state, as reported by [`Tenanted::stats`].
#[derive(Debug, Clone)]
pub struct TenantStats {
    /// The tenant.
    pub tenant: TenantId,
    /// Whether its resource is built (`false` = creation in flight).
    pub ready: bool,
    /// Time since its last use.
    pub idle: Duration,
}

/// What one [`Tenanted::sweep`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Resources evicted for being idle.
    pub idle_evicted: usize,
    /// Resources evicted to stay under `max-active`.
    pub lru_evicted: usize,
    /// Expired negative-cache entries dropped.
    pub negative_purged: usize,
}

impl SweepReport {
    /// Whether the sweep removed anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// An unwired map — what [`PerTenant::install`](crate::PerTenant) provides
    /// before the source bean exists. Resolving through it fails with
    /// [`TenantError::NoSource`].
    #[must_use]
    pub fn unwired() -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: DashMap::new(),
                negative: DashMap::new(),
                wiring: Late::new(),
                epoch: Instant::now(),
                trimming: AtomicBool::new(false),
                counters: Counters::default(),
            }),
        }
    }

    /// A wired map.
    ///
    /// The [`PerTenant`](crate::PerTenant) plugin is the normal way to get one;
    /// this constructor is for tests and for embedding the map in something else.
    /// `beans` backs [`TenantContext::bean`] and the cascade — pass
    /// `Arc::new(BeanContext::empty())` when the source needs neither.
    #[must_use]
    pub fn new(
        source: Arc<dyn TenantSource<T>>,
        beans: Arc<BeanContext>,
        settings: TenantedSettings,
        fallback: Option<T>,
    ) -> Self {
        let map = Self::unwired();
        map.wire(source, beans, settings, fallback);
        map
    }

    /// Fill an unwired map, returning `false` if it was already wired.
    ///
    /// The other half of the install/configure split: [`unwired`](Self::unwired)
    /// puts the bean in the state early (so controllers compile against it), and
    /// this fills in the source once the graph exists. Every clone of the map
    /// shares one interior, so wiring any handle wires the bean the state holds.
    /// The [`PerTenant`](crate::PerTenant) plugin does this for you; call it
    /// directly only when embedding a map outside the builder.
    pub fn wire(
        &self,
        source: Arc<dyn TenantSource<T>>,
        beans: Arc<BeanContext>,
        settings: TenantedSettings,
        fallback: Option<T>,
    ) -> bool {
        self.inner
            .wiring
            .fill(Wiring {
                source,
                beans,
                settings,
                fallback,
            })
            .is_ok()
    }

    /// The resource for `tenant`, creating it on first use.
    ///
    /// Concurrent callers for the same cold tenant share one `create` call.
    pub async fn get(&self, tenant: &TenantId) -> Result<T, TenantError> {
        self.resolve(tenant, ResolutionChain::root::<T>()).await
    }

    /// The already-built resource for `tenant`, without creating anything.
    #[must_use]
    pub fn peek(&self, tenant: &TenantId) -> Option<T> {
        let slot = self.inner.slots.get(tenant)?.clone();
        let value = slot.cell.get().cloned();
        if value.is_some() {
            self.touch(&slot);
        }
        value
    }

    /// Tenants with a live resource.
    #[must_use]
    pub fn active(&self) -> Vec<TenantId> {
        self.inner
            .slots
            .iter()
            .filter(|entry| entry.value().cell.get().is_some())
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Per-tenant readiness and idle time, including creations in flight.
    #[must_use]
    pub fn stats(&self) -> Vec<TenantStats> {
        let now = self.now_millis();
        self.inner
            .slots
            .iter()
            .map(|entry| TenantStats {
                tenant: entry.key().clone(),
                ready: entry.value().cell.get().is_some(),
                idle: Duration::from_millis(
                    now.saturating_sub(entry.value().last_used.load(Ordering::Relaxed)),
                ),
            })
            .collect()
    }

    /// Live counters.
    #[must_use]
    pub fn metrics(&self) -> TenantedMetrics {
        let c = &self.inner.counters;
        TenantedMetrics {
            active: self.active().len(),
            negative: self.inner.negative.len(),
            hits: c.hits.load(Ordering::Relaxed),
            created: c.created.load(Ordering::Relaxed),
            create_failures: c.create_failures.load(Ordering::Relaxed),
            timeouts: c.timeouts.load(Ordering::Relaxed),
            unknown: c.unknown.load(Ordering::Relaxed),
            fallbacks: c.fallbacks.load(Ordering::Relaxed),
            disposed: c.disposed.load(Ordering::Relaxed),
            evicted_idle: c.evicted_idle.load(Ordering::Relaxed),
            evicted_lru: c.evicted_lru.load(Ordering::Relaxed),
        }
    }

    /// The statuses tenancy failures from this map map to.
    #[must_use]
    pub fn statuses(&self) -> TenantStatuses {
        self.settings().statuses
    }

    /// The effective settings (defaults while unwired).
    #[must_use]
    pub fn settings(&self) -> TenantedSettings {
        self.inner
            .wiring
            .get()
            .map_or_else(TenantedSettings::default, |w| w.settings)
    }

    /// Drop a tenant's resource and **await** its disposal.
    ///
    /// Returns `false` when the tenant had nothing cached. Use this for a
    /// deliberate teardown (a tenant was offboarded); it is also what the idle
    /// and LRU sweeps call.
    pub async fn evict(&self, tenant: &TenantId) -> bool {
        let Some((_, slot)) = self.inner.slots.remove(tenant) else {
            return false;
        };
        self.dispose(tenant, &slot).await;
        true
    }

    /// Drop a tenant's cached resource **now**, disposing of it in the
    /// background.
    ///
    /// The synchronous form of [`evict`](Self::evict), for rotation: the next
    /// request rebuilds from the source (a fresh DSN, new credentials) while the
    /// old resource closes behind it. Also clears any negative-cache entry, so a
    /// tenant that was just provisioned is retried immediately.
    pub fn invalidate(&self, tenant: &TenantId) -> bool {
        self.inner.negative.remove(tenant);
        let Some((_, slot)) = self.inner.slots.remove(tenant) else {
            return false;
        };
        if slot.cell.get().is_some() {
            let this = self.clone();
            let disposing = tenant.clone();
            if !spawn_detached(async move {
                this.dispose(&disposing, &slot).await;
            }) {
                tracing::debug!(
                    tenant = %tenant,
                    "invalidate() outside a Tokio runtime: dropping the resource without dispose"
                );
            }
        }
        true
    }

    /// Create the resources for `tenants` up front (the plugin's `eager` list).
    ///
    /// Sequential on purpose — a warmup should not open every tenant's pool at
    /// once. Returns the tenants that failed, so a caller can decide whether a
    /// cold tenant is fatal.
    pub async fn preload<I>(&self, tenants: I) -> Vec<(TenantId, TenantError)>
    where
        I: IntoIterator<Item = TenantId>,
    {
        let mut failures = Vec::new();
        for tenant in tenants {
            if let Err(err) = self.get(&tenant).await {
                failures.push((tenant, err));
            }
        }
        failures
    }

    /// Dispose of every cached resource. Called at shutdown.
    pub async fn drain(&self) {
        let entries: Vec<(TenantId, Arc<Slot<T>>)> = self
            .inner
            .slots
            .iter()
            .map(|e| (e.key().clone(), Arc::clone(e.value())))
            .collect();
        for (tenant, _) in &entries {
            self.inner.slots.remove(tenant);
        }
        self.inner.negative.clear();
        for (tenant, slot) in &entries {
            self.dispose(tenant, slot).await;
        }
    }

    /// Evict idle resources, trim to `max-active`, purge the negative cache.
    ///
    /// What the background [`ServiceComponent`](r2e_core::service::ServiceComponent)
    /// runs on a timer; call it directly from an admin endpoint or a test to
    /// sweep deterministically.
    pub async fn sweep(&self) -> SweepReport {
        let settings = self.settings();
        let mut report = SweepReport {
            negative_purged: self.purge_negative(&settings),
            ..SweepReport::default()
        };

        // `checked_sub`, not `saturating_sub`: a map younger than the TTL has
        // nothing idle in it, and clamping the cutoff to 0 would match slots
        // touched in the map's first millisecond.
        if let Some(cutoff) = settings
            .idle_ttl
            .and_then(|ttl| self.now_millis().checked_sub(ttl.as_millis() as u64))
        {
            for tenant in self.idle_since(cutoff) {
                if self.evict(&tenant).await {
                    report.idle_evicted += 1;
                    self.inner.counters.evicted_idle.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        report.lru_evicted = self.trim_to_max_active(&settings).await;
        report
    }

    // ── internals ───────────────────────────────────────────────────────────

    pub(crate) async fn resolve(
        &self,
        tenant: &TenantId,
        chain: ResolutionChain,
    ) -> Result<T, TenantError> {
        let Some(wiring) = self.inner.wiring.get() else {
            return Err(TenantError::NoSource(std::any::type_name::<T>()));
        };

        if self.negative_hit(tenant, &wiring.settings) {
            return self.unknown_or_fallback(tenant, wiring);
        }

        let slot = self.slot_for(tenant);
        self.touch(&slot);
        if let Some(value) = slot.cell.get() {
            self.inner.counters.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(value.clone());
        }

        // The `Arc<Slot>` is already cloned out of the map: no shard guard is
        // held across the awaits below.
        let outcome = slot
            .cell
            .get_or_try_init(|| async {
                let ctx = TenantContext::new(tenant, Arc::clone(&wiring.beans), chain);
                let creating = wiring.source.create(tenant, &ctx);
                let created = match wiring.settings.create_timeout {
                    Some(budget) => match tokio::time::timeout(budget, creating).await {
                        Ok(created) => created,
                        Err(_) => {
                            self.inner.counters.timeouts.fetch_add(1, Ordering::Relaxed);
                            return Err(CreateFailure::Failed(TenantError::Timeout(
                                tenant.clone(),
                            )));
                        }
                    },
                    None => creating.await,
                };
                match created {
                    Ok(Some(value)) => {
                        self.inner.negative.remove(tenant);
                        self.inner.counters.created.fetch_add(1, Ordering::Relaxed);
                        Ok(value)
                    }
                    Ok(None) => {
                        self.inner.counters.unknown.fetch_add(1, Ordering::Relaxed);
                        self.remember_negative(tenant, &wiring.settings);
                        Err(CreateFailure::Unknown)
                    }
                    Err(cause) => {
                        self.inner
                            .counters
                            .create_failures
                            .fetch_add(1, Ordering::Relaxed);
                        Err(CreateFailure::Failed(classify(tenant, cause)))
                    }
                }
            })
            .await;

        match outcome {
            Ok(value) => {
                let value = value.clone();
                self.touch(&slot);
                self.enforce_max_active(&wiring.settings);
                Ok(value)
            }
            Err(failure) => {
                // Nothing is cached on failure. `ptr_eq` keeps a concurrent
                // retry's fresh slot from being removed by this one's cleanup.
                self.inner
                    .slots
                    .remove_if(tenant, |_, current| Arc::ptr_eq(current, &slot));
                match failure {
                    CreateFailure::Failed(err) => Err(err),
                    CreateFailure::Unknown => self.unknown_or_fallback(tenant, wiring),
                }
            }
        }
    }

    fn unknown_or_fallback(&self, tenant: &TenantId, wiring: &Wiring<T>) -> Result<T, TenantError> {
        match &wiring.fallback {
            Some(default) => {
                self.inner.counters.fallbacks.fetch_add(1, Ordering::Relaxed);
                Ok(default.clone())
            }
            None => Err(TenantError::Unknown(tenant.clone())),
        }
    }

    fn slot_for(&self, tenant: &TenantId) -> Arc<Slot<T>> {
        // Clone the `Arc` out and drop the guard before returning: every caller
        // awaits, and awaiting under a DashMap guard deadlocks the shard.
        if let Some(existing) = self.inner.slots.get(tenant) {
            return Arc::clone(existing.value());
        }
        Arc::clone(
            self.inner
                .slots
                .entry(tenant.clone())
                .or_insert_with(|| {
                    Arc::new(Slot {
                        cell: OnceCell::new(),
                        last_used: AtomicU64::new(self.now_millis()),
                    })
                })
                .value(),
        )
    }

    async fn dispose(&self, tenant: &TenantId, slot: &Slot<T>) {
        let Some(wiring) = self.inner.wiring.get() else {
            return;
        };
        let Some(value) = slot.cell.get() else {
            return;
        };
        self.inner.counters.disposed.fetch_add(1, Ordering::Relaxed);
        wiring.source.dispose(tenant, value.clone()).await;
    }

    fn now_millis(&self) -> u64 {
        self.inner.epoch.elapsed().as_millis() as u64
    }

    fn touch(&self, slot: &Slot<T>) {
        slot.last_used.store(self.now_millis(), Ordering::Relaxed);
    }

    fn negative_hit(&self, tenant: &TenantId, settings: &TenantedSettings) -> bool {
        let Some(ttl) = settings.negative_ttl else {
            return false;
        };
        let Some(entry) = self.inner.negative.get(tenant) else {
            return false;
        };
        let recorded = *entry.value();
        drop(entry);
        if self.now_millis().saturating_sub(recorded) < ttl.as_millis() as u64 {
            true
        } else {
            self.inner.negative.remove(tenant);
            false
        }
    }

    fn remember_negative(&self, tenant: &TenantId, settings: &TenantedSettings) {
        if settings.negative_ttl.is_none() {
            return;
        }
        if self.inner.negative.len() >= settings.max_negative {
            self.purge_negative(settings);
        }
        if self.inner.negative.len() < settings.max_negative {
            self.inner.negative.insert(tenant.clone(), self.now_millis());
        }
    }

    fn purge_negative(&self, settings: &TenantedSettings) -> usize {
        let Some(ttl) = settings.negative_ttl else {
            let purged = self.inner.negative.len();
            self.inner.negative.clear();
            return purged;
        };
        let now = self.now_millis();
        let ttl = ttl.as_millis() as u64;
        let before = self.inner.negative.len();
        self.inner
            .negative
            .retain(|_, recorded| now.saturating_sub(*recorded) < ttl);
        before - self.inner.negative.len()
    }

    fn idle_since(&self, cutoff: u64) -> Vec<TenantId> {
        self.inner
            .slots
            .iter()
            .filter(|entry| {
                entry.value().cell.get().is_some()
                    && entry.value().last_used.load(Ordering::Relaxed) <= cutoff
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    async fn trim_to_max_active(&self, settings: &TenantedSettings) -> usize {
        let mut evicted = 0;
        for tenant in self.lru_victims(settings.max_active) {
            if self.evict(&tenant).await {
                evicted += 1;
                self.inner.counters.evicted_lru.fetch_add(1, Ordering::Relaxed);
            }
        }
        evicted
    }

    /// The least-recently-used ready tenants above `max_active`.
    fn lru_victims(&self, max_active: usize) -> Vec<TenantId> {
        let mut ready: Vec<(u64, TenantId)> = self
            .inner
            .slots
            .iter()
            .filter(|entry| entry.value().cell.get().is_some())
            .map(|entry| {
                (
                    entry.value().last_used.load(Ordering::Relaxed),
                    entry.key().clone(),
                )
            })
            .collect();
        if ready.len() <= max_active {
            return Vec::new();
        }
        ready.sort_by_key(|(last_used, _)| *last_used);
        let excess = ready.len() - max_active;
        ready.into_iter().take(excess).map(|(_, id)| id).collect()
    }

    /// Keep the cap even with no sweeper running: one background trim at a
    /// time, and only when the map is actually over its limit.
    fn enforce_max_active(&self, settings: &TenantedSettings) {
        if self.inner.slots.len() <= settings.max_active {
            return;
        }
        if self
            .inner
            .trimming
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let this = self.clone();
        let settings = *settings;
        if !spawn_detached(async move {
            this.trim_to_max_active(&settings).await;
            this.inner.trimming.store(false, Ordering::Release);
        }) {
            self.inner.trimming.store(false, Ordering::Release);
        }
    }
}

/// Outcome of a failed `create`, kept internal so the `OnceCell` initializer can
/// distinguish "unknown tenant" (which may still fall back) from a real error.
enum CreateFailure {
    Failed(TenantError),
    Unknown,
}

/// Classify a `create` failure.
///
/// A cascading source reaches its dependencies with `ctx.get::<U>()?`, which
/// boxes a [`TenantError`]. Re-wrapping that as `Unavailable` would turn a
/// missing `PerTenant` plugin or a dependency cycle (500-class wiring bugs) into
/// a retryable 503 and bury the chain one `source()` hop deeper, so a boxed
/// `TenantError` keeps its own classification. Every other cause is a genuine
/// provisioning failure for *this* tenant.
fn classify(tenant: &TenantId, cause: BoxError) -> TenantError {
    match cause.downcast::<TenantError>() {
        Ok(inner) => *inner,
        Err(cause) => TenantError::unavailable(tenant.clone(), cause),
    }
}

/// Spawn a detached task, reporting whether a runtime was available.
fn spawn_detached<F>(future: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return false;
    }
    // Dropping the handle detaches the task, which is the point: the caller is
    // a synchronous path (`Drop`, `invalidate`) that cannot await disposal.
    drop(r2e_core::rt::spawn(future));
    true
}

/// The background sweeper.
///
/// Wired by the [`PerTenant`](crate::PerTenant) plugin — the same shape as
/// `DbPool`'s reaper: one task per map, driven by the app's shutdown token, and
/// draining every tenant's resource when that token is cancelled.
impl<T> r2e_core::service::ServiceComponent for Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Deps = TCons<Self, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Self>()
    }

    async fn start(self, shutdown: CancellationToken) {
        let interval = self.settings().sweep_interval();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    self.drain().await;
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    let report = self.sweep().await;
                    if !report.is_empty() {
                        tracing::debug!(
                            resource = std::any::type_name::<T>(),
                            idle_evicted = report.idle_evicted,
                            lru_evicted = report.lru_evicted,
                            negative_purged = report.negative_purged,
                            "swept per-tenant resources"
                        );
                    }
                }
            }
        }
    }
}

impl<T> std::fmt::Debug for Tenanted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tenanted")
            .field("resource", &std::any::type_name::<T>())
            .field("slots", &self.inner.slots.len())
            .field("wired", &self.inner.wiring.get().is_some())
            .finish()
    }
}
