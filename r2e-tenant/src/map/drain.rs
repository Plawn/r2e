//! Shutdown: the draining latch, and the drain that waits for everything it
//! is draining to be closed.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::error::TenantError;
use crate::TenantId;

use super::state::{Slot, Tenanted};

#[allow(unused_imports)]
use super::dispose::Pending;
#[allow(unused_imports)]
use super::state::Inner;

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Dispose of every cached resource. Called at shutdown.
    ///
    /// Draining **latches**: the map is closed for business afterwards, and
    /// every later `resolve` fails with a `503` rather than opening a resource
    /// nobody will close. Without that, a request arriving mid-shutdown could
    /// repopulate the map behind the drain forever. Creations already in flight
    /// are not aborted — they finish, notice the latch, and dispose of what they
    /// built instead of caching it.
    ///
    /// Removal is conditional on slot identity, so a slot installed by a
    /// concurrent retry is never removed by this drain without being disposed
    /// of, and the pass repeats until no ready slot is left.
    ///
    /// # When this returns, everything is closed
    ///
    /// Walking the map is not enough on its own, because a live value can be
    /// *outside* it while still needing to be closed:
    ///
    /// - a resolve holding a slot that was detached under it (a cancelled
    ///   initializer removes its own empty slot without bumping the epoch; the
    ///   waiter that inherits the cell fills it and only then reattaches), and
    /// - a disposal somebody has committed but not yet awaited — including the
    ///   one that same resolve spawns when it finds itself `Orphaned`.
    ///
    /// Both mint a [`Pending`], so this waits for [`Inner::in_flight`] to reach
    /// zero as well as for the map to come up empty, and re-passes whenever
    /// either is not settled. The counter is read **before** each pass, never
    /// after: a resolve that reattaches at `t1` decrements at `t2 > t1`, so a
    /// count read after the pass could be zero while the slot it just re-mapped
    /// is still there. Read first, walk second, and a zero reading means every
    /// slot that will ever be mapped already was when the pass ran.
    ///
    /// # Termination, and why a flood of 503s cannot hold it open
    ///
    /// `resolve` admits work through a **double check**: it reads the latch
    /// once *without touching the counter*, increments only if that read was
    /// clear, and re-reads before proceeding. So a resolve arriving after the
    /// latch store neither starts work nor counts — it is rejected by the first
    /// read and never appears here. The only resolves that ever increment are
    /// the finite set that was already past that first read when the latch went
    /// up, and each of them either passes the second read (and is then waited
    /// on, by the `SeqCst` argument at the admission site) or decrements again
    /// at once.
    ///
    /// Increments are therefore finite and each is paired with a decrement, so
    /// the counter strictly drains to zero. That is what makes notifying only on
    /// the zero *transition* sufficient: [`Notified`](tokio::sync::Notify) is
    /// registered before the count is read, so a transition after the read wakes
    /// this loop, and a transition before it is either already reflected in the
    /// read or followed by another one — a final state above zero is impossible.
    /// Each in-flight resolve classifies its slot once and each debt is
    /// discharged once, so every pass strictly reduces what is left.
    ///
    /// There is no timeout, deliberately — the rest of `drain` has none either,
    /// and a source's `dispose` is the thing the caller asked to wait for. What
    /// this does *not* cover is a concurrent `invalidate`/`evict` call, which is
    /// not fenced by the latch; racing a manual removal against shutdown is
    /// outside the invariant.
    pub async fn drain(&self) {
        // Latch first, fence second. The latch is the half the termination
        // argument rests on (it is what closes admission — see `resolve` — and
        // it pairs `SeqCst` with the in-flight counter), so it goes up as early
        // as possible. The epoch bump is defence in depth: it turns "reattach,
        // then get drained by the next pass" into "orphan immediately".
        //
        // Both windows between the two are harmless *because of the counter*,
        // not because of any ordering between them — there is none to have
        // across runtime workers, and "no await separates them" was never a
        // serialization argument. Latch-then-fence: a resolve may restore its
        // slot at the old epoch, and a later pass removes it. Fence-then-latch:
        // a resolve may orphan and commit, and that commit mints a debt inside
        // the shard-lock critical section. Either way the work is counted before
        // this can stop looping.
        self.inner.draining.store(true, Ordering::SeqCst);
        self.bump_epoch();
        self.inner.negative.clear();

        loop {
            // Register for the wake-up *before* observing anything, so a
            // counter that hits zero between here and the await is not a lost
            // wakeup.
            let settled = self.inner.settled.notified();
            tokio::pin!(settled);
            settled.as_mut().enable();

            // Before the pass — see the rustdoc.
            let quiet = self.inner.in_flight.load(Ordering::SeqCst) == 0;

            let ready: Vec<(TenantId, Arc<Slot<T>>)> = self
                .inner
                .slots
                .iter()
                .filter(|entry| entry.value().is_ready())
                .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
                .collect();
            for (tenant, slot) in &ready {
                // `ptr_eq` inside `take_slot`: a recreate's replacement slot is
                // left where it is (and picked up by the next pass) instead of
                // being dropped on the floor undisposed. The gate is taken in
                // the same critical section as the removal.
                if let Some(debt) = self.take_slot(tenant, slot) {
                    self.run_committed_dispose(tenant, slot, debt).await;
                }
            }
            if !ready.is_empty() {
                continue;
            }
            // Nothing mapped, and nothing was in flight when this pass started:
            // whatever was going to be mapped already was, and every committed
            // disposal has completed.
            if quiet && self.inner.in_flight.load(Ordering::SeqCst) == 0 {
                break;
            }
            settled.await;
        }
        self.inner.negative.clear();
    }

    pub(super) fn is_draining(&self) -> bool {
        // `SeqCst`, paired with the `SeqCst` increment in [`Pending::new`]: the
        // two sides are a store-buffer shape (drain stores the latch then reads
        // the counter; a resolve increments the counter then reads the latch),
        // and release/acquire lets *both* of them miss. See `drain`.
        self.inner.draining.load(Ordering::SeqCst)
    }
}

/// What a request gets once [`Tenanted::drain`] has latched: a retryable 503,
/// the same class as "the tenant's resource could not be built right now".
pub(super) fn draining_error(tenant: &TenantId) -> TenantError {
    TenantError::unavailable(
        tenant.clone(),
        "the per-tenant resource map is draining (shutdown)".into(),
    )
}
