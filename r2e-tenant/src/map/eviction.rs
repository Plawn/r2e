//! Reclaim: the sweep, the idle and LRU trims, and the background sweeper
//! that drives them.

use std::sync::atomic::Ordering;

use r2e_core::rt::{self, CancelToken};
use r2e_core::type_list::{TCons, TNil};
use r2e_core::BeanContext;

use crate::TenantId;

use super::dispose::spawn_detached;
use super::state::Tenanted;
use super::{SweepReport, TenantedSettings};

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Evict idle resources, trim to `max-active`, purge the negative cache.
    ///
    /// What the background [`ServiceComponent`](r2e_core::runtime::service::ServiceComponent)
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

    /// How many tenants have a **built** resource.
    ///
    /// What `max-active` is really about: `slots.len()` also counts creations in
    /// flight, which no trim can evict.
    fn ready_count(&self) -> usize {
        self.inner
            .slots
            .iter()
            .filter(|entry| entry.value().is_ready())
            .count()
    }

    fn idle_since(&self, cutoff: u64) -> Vec<TenantId> {
        self.inner
            .slots
            .iter()
            .filter(|entry| {
                entry.value().is_ready()
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
            .filter(|entry| entry.value().is_ready())
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
    ///
    /// The trim **loops**, and clears its flag before re-checking. Completions
    /// that arrive while a trim is running see `trimming = true` and return
    /// without scheduling anything, so a trim that snapshotted too few ready
    /// slots would otherwise finish and leave the map over the cap until the
    /// periodic sweep. Clearing the flag *first* and only then re-reading the
    /// map closes that handoff window: whatever the completion did or did not
    /// schedule, one of the two sides sees the excess.
    ///
    /// The re-check counts **ready** slots, not `slots.len()`, and it runs
    /// unconditionally — including after a round that evicted nothing, which is
    /// exactly the case the handoff race lives in (the last pass saw no ready
    /// excess, then a creation completed while the flag was still up). A pass
    /// over a ready excess always evicts at least one slot, so re-taking the
    /// flag there terminates.
    ///
    /// Residual, deliberately left to the periodic sweep and to the completing
    /// creations themselves: `slots.len()` can stay over the cap while
    /// `ready_count()` does not, because every excess slot is still being
    /// created and nothing can evict a creation in flight.
    pub(super) fn enforce_max_active(&self, settings: &TenantedSettings) {
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
            loop {
                // One round: trim until a pass finds nothing left to evict.
                while this.trim_to_max_active(&settings).await > 0 {}
                // Clear, *then* re-read: a completion that declined to schedule
                // while this round ran is picked up here, whether or not the
                // round itself evicted anything.
                this.inner.trimming.store(false, Ordering::Release);
                if this.ready_count() <= settings.max_active {
                    break;
                }
                // Let whoever made those slots ready make progress before
                // another round: a pass that evicts nothing never awaits.
                rt::yield_now().await;
                if this
                    .inner
                    .trimming
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    // Someone else took the flag between the store and here:
                    // they own the excess now.
                    break;
                }
            }
        }) {
            self.inner.trimming.store(false, Ordering::Release);
        }
    }
}

/// The background sweeper.
///
/// Wired by the [`PerTenant`](crate::PerTenant) plugin — the same shape as
/// `DbPool`'s reaper: one task per map, driven by the app's shutdown token, and
/// draining every tenant's resource when that token is cancelled.
impl<T> r2e_core::runtime::service::ServiceComponent for Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Deps = TCons<Self, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Self>()
    }

    // The one raw-token touchpoint left in this crate: `ServiceComponent::start`
    // is still typed on `tokio_util::sync::CancellationToken`, because
    // `#[derive(BackgroundService)]` emits that type into user code and flipping
    // it is a user-visible break owned by phase 2e/2f of
    // `plans/runtime-http-dependency-containment.md`. Convert at the boundary so
    // the body stays on the facade.
    async fn start(self, shutdown: tokio_util::sync::CancellationToken) {
        let shutdown = CancelToken::from(shutdown);
        let interval = self.settings().sweep_interval();
        loop {
            rt::select! {
                _ = shutdown.cancelled() => {
                    self.drain().await;
                    break;
                }
                _ = rt::sleep(interval) => {
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
