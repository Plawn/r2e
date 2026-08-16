//! The public front door: construction, lookup, the read-only views, and
//! the two manual removals.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use r2e_core::plugin::GraphHandle;
use tokio::sync::Notify;

use crate::error::{TenantError, TenantStatuses};
use crate::source::{ResolutionChain, TenantSource};
use crate::TenantId;

use super::state::{Counters, Inner, Tenanted, Wiring};
use super::{TenantStats, TenantedMetrics, TenantedSettings};

#[allow(unused_imports)]
use crate::source::TenantContext;

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// A wired map.
    ///
    /// The [`PerTenant`](crate::PerTenant) plugin is the normal way to get one
    /// (its `build` calls this with the graph handle the framework fills after
    /// `build_state()`); this constructor is for tests and for embedding the
    /// map in something else. `graph` backs [`TenantContext::bean`] and the
    /// cascade — pass [`GraphHandle::default()`] when the source needs
    /// neither, or fill your own handle once your `BeanContext` exists.
    ///
    /// # Panics
    ///
    /// Panics when `settings.max_active` is `0`. A cap of zero would create
    /// every resource and immediately evict it; it is a misconfiguration, not a
    /// way to disable the map.
    #[must_use]
    pub fn new(
        source: Arc<dyn TenantSource<T>>,
        graph: GraphHandle,
        settings: TenantedSettings,
        fallback: Option<T>,
    ) -> Self {
        assert!(
            settings.max_active > 0,
            "`max-active` must be at least 1 for `Tenanted<{}>`: a cap of 0 would create every \
             resource and evict it straight away. Use `PerTenant::max_active(n)` / \
             `tenancy.max-active: n` with n >= 1.",
            std::any::type_name::<T>()
        );
        Self {
            inner: Arc::new(Inner {
                slots: DashMap::new(),
                negative: DashMap::new(),
                wiring: Wiring {
                    source,
                    graph,
                    settings,
                    fallback,
                },
                started: Instant::now(),
                epoch: AtomicU64::new(0),
                trimming: AtomicBool::new(false),
                draining: AtomicBool::new(false),
                in_flight: AtomicUsize::new(0),
                settled: Notify::new(),
                counters: Counters::default(),
            }),
        }
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
            .filter(|entry| entry.value().is_ready())
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
                ready: entry.value().is_ready(),
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

    /// The effective settings.
    #[must_use]
    pub fn settings(&self) -> TenantedSettings {
        self.inner.wiring.settings
    }

    /// Drop a tenant's resource and **await** its disposal.
    ///
    /// Returns `false` when the tenant had nothing **ready**. Use this for a
    /// deliberate teardown (a tenant was offboarded); it is also what the idle
    /// and LRU sweeps call.
    ///
    /// A creation in flight is deliberately left alone (and reported `false`):
    /// detaching an empty slot would let the creation finish into a slot the map
    /// no longer holds, handing the caller a value that is never disposed of.
    /// Evict again once it is ready.
    ///
    /// **It really awaits the closure.** The disposal gate is committed inside
    /// [`take_ready`](Self::take_ready)'s shard-lock critical section, so a
    /// participant that reaches [`reattach`](Self::reattach) a moment later
    /// cannot take the value's disposal off this call and onto a detached task:
    /// it reads the committed gate under that same lock and stands down. The one
    /// case where this returns `true` without awaiting anything is a slot that
    /// was *already* someone else's to close — which no public path can produce,
    /// for the reason spelled out on `take_ready`.
    pub async fn evict(&self, tenant: &TenantId) -> bool {
        let Some(removed) = self.take_ready(tenant) else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.run_committed_dispose(tenant, &removed.slot, debt).await;
        }
        true
    }

    /// Drop a tenant's cached resource **now**, disposing of it in the
    /// background.
    ///
    /// The synchronous form of [`evict`](Self::evict), for rotation: the next
    /// request rebuilds from the source (a fresh DSN, new credentials) while the
    /// old resource closes behind it. Also clears any negative-cache entry, so a
    /// tenant that was just provisioned is retried immediately.
    ///
    /// Two things the caller has to know, because this cannot await:
    /// - `true` means the resource was **removed and disposal was spawned**, not
    ///   that disposal finished. Use [`evict`](Self::evict) when you need the
    ///   pool to be closed before you return.
    /// - Outside a Tokio runtime there is nothing to spawn on: the resource is
    ///   dropped *without* `dispose` (a `debug!` records it). Every in-process
    ///   caller of `invalidate` is inside the runtime; this only bites in a
    ///   synchronous test harness.
    ///
    /// Like `evict`, a creation in flight is left alone — it stays mapped and
    /// caches what it builds. That creation **overlaps** the invalidation and is
    /// deliberately not fenced: removal never touches it, so it keeps the slot
    /// the map owns. What is fenced off is the opposite case: a creation that was
    /// already **detached** when this ran cannot write its pre-invalidation value
    /// (or its "unknown" verdict) back into the map afterwards.
    ///
    /// # Order
    ///
    /// `take_ready` runs **before** the negative entry is cleared, and never the
    /// other way around. `take_ready` bumps the epoch before it takes the key's
    /// shard lock, so a detached `Ok(None)` writeback either
    ///
    /// - held the shard lock first and inserted its memo — which the
    ///   `negative.remove` below, sequenced after, then clears; or
    /// - takes the lock after the removal, reads the bumped epoch under it, and
    ///   never inserts.
    ///
    /// Clearing first would leave a third case open: the memo lands *after* the
    /// clear and survives the invalidate for a whole `negative-ttl`.
    pub fn invalidate(&self, tenant: &TenantId) -> bool {
        let removed = self.take_ready(tenant);
        self.inner.negative.remove(tenant);
        let Some(removed) = removed else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.spawn_committed_dispose(tenant, &removed.slot, debt);
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
}
