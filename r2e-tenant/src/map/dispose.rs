//! Disposal: the one-shot gate, the removal primitives that commit it under
//! the key's shard lock, and the in-flight accounting `drain` waits on.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use dashmap::mapref::entry::Entry;

use crate::TenantId;

use super::state::{Inner, Slot, Tenanted};

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Unmap the slot (only when it is still this one) and dispose of its value.
    ///
    /// The gate is taken under the shard lock by
    /// [`take_slot`](Self::take_slot); losing it means someone else owns the
    /// await and this caller must not duplicate it.
    pub(super) async fn detach_and_dispose(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) {
        if let Some(debt) = self.take_slot(tenant, slot) {
            self.run_committed_dispose(tenant, slot, debt).await;
        }
    }

    /// Spawn the await half of a disposal this caller already committed.
    ///
    /// Outside a Tokio runtime there is nothing to spawn on. The gate stays
    /// taken: the slot is already out of the map (or was never the map's), so
    /// nobody can reach it to retry, and a second commit would only risk a
    /// double dispose the moment a runtime does exist.
    pub(super) fn spawn_committed_dispose(&self, tenant: &TenantId, slot: &Arc<Slot<T>>, debt: DisposalDebt<T>) {
        let this = self.clone();
        let disposing = tenant.clone();
        let slot = Arc::clone(slot);
        // The debt moves into the future: it keeps the work counted until the
        // spawned task finishes, and if there is nothing to spawn on it is
        // dropped with the future, so the counter never gets stuck.
        if !spawn_detached(async move {
            this.run_committed_dispose(&disposing, &slot, debt).await;
        }) {
            tracing::debug!(
                tenant = %tenant,
                resource = std::any::type_name::<T>(),
                "no Tokio runtime to dispose on: dropping the resource without dispose"
            );
        }
    }

    /// Remove a tenant's slot, but only once its resource is built — and take
    /// its disposal gate in the same breath.
    ///
    /// The one removal primitive every caller goes through, so "an in-flight
    /// creation is never detached from the map" holds for eviction, the sweeps
    /// and `invalidate` alike — and so does the epoch fence, which is bumped
    /// here for all of them.
    ///
    /// # Why the gate is committed inside the predicate
    ///
    /// `remove_if`'s predicate runs **under the key's shard lock**, which is the
    /// same lock [`reattach`](Self::reattach) decides under. Committing there
    /// rather than after the lock is released is what keeps an awaited remover
    /// from losing its own value: a late participant that reaches `reattach`
    /// afterwards reads the committed gate under that lock and spawns nothing,
    /// so `evict().await` really has closed the resource by the time it returns.
    /// Deferring the commit to `dispose` would leave a window where the
    /// participant wins the CAS, detaches the disposal onto a spawned task, and
    /// the remover returns early.
    ///
    /// # The already-committed edge
    ///
    /// A **mapped** slot cannot have a committed gate. The argument is over the
    /// six commit sites rather than over intentions: each one holds this key's
    /// shard lock, and none of them ends its critical section with the slot it
    /// committed still under the key.
    ///
    /// - `take_ready` here and `take_slot`'s occupied-and-ours branch commit and
    ///   **remove** in the same section;
    /// - `take_slot`'s vacant and occupied-by-another branches, and both of
    ///   `reattach`'s `Orphaned` arms, commit a slot that is *not* the one under
    ///   the key (there is none, or it is a different `Arc`).
    ///
    /// And the only way a slot becomes mapped again is `reattach`'s vacant
    /// restore, which reads `is_disposed()` under this same guard and refuses.
    /// So the two states never meet.
    ///
    /// Should it happen anyway — a test seam, or future code — the slot is still
    /// removed (it is dying either way) and the disposal is **skipped**: the
    /// committer owns that await, and a remover cannot await a disposal it does
    /// not own. Logged at debug rather than asserted, because the safe action
    /// and the loud action differ here and the safe one wins.
    pub(super) fn take_ready(&self, tenant: &TenantId) -> Option<Removed<T>> {
        self.bump_epoch();
        let mut debt = None;
        let (_, slot) = self.inner.slots.remove_if(tenant, |_, slot| {
            if !slot.is_ready() {
                return false;
            }
            // MUST stay inside this predicate: it runs under the key's shard
            // lock, which is the only thing ordering it against a `reattach`.
            debt = self.commit_dispose(slot);
            true
        })?;
        if debt.is_none() {
            tracing::debug!(
                tenant = %tenant,
                resource = std::any::type_name::<T>(),
                "removed a slot whose disposal was already owned by someone else"
            );
        }
        Some(Removed { slot, debt })
    }

    /// Unmap `slot` when it is still the one under `tenant`, committing its
    /// disposal gate in the same critical section. Returns whether **this**
    /// caller owes the value its `dispose` await.
    ///
    /// The identity-conditional twin of [`take_ready`](Self::take_ready), for
    /// the paths that already hold the slot they mean to remove: `drain` and the
    /// draining escape in `resolve`. Unlike `take_ready` it also commits a slot
    /// that is *no longer mapped* — the caller is holding a value that has to be
    /// closed either way — and that is exactly why the whole thing goes through
    /// `entry()` rather than `remove_if` plus a bare CAS.
    ///
    /// # Every branch commits under the key's entry guard
    ///
    /// [`reattach`](Self::reattach) also goes through `entry()` on this key, so
    /// the guard is the serialization point between "this value is being closed"
    /// and "this value is going back into the map":
    ///
    /// - **occupied by this slot** — commit and remove in one section. The
    ///   remover owns both, and no reattach can interleave.
    /// - **vacant** — commit while still *holding* the vacant guard. A reattach
    ///   arriving afterwards takes the same guard, reads `is_disposed()` and
    ///   refuses; one arriving before has re-mapped the slot, which is the
    ///   occupied case above. Dropping the guard before the CAS is what let a
    ///   restore slip in between and steal the disposal onto a detached task.
    /// - **occupied by a different slot** — the key is somebody else's, so
    ///   nothing is removed, but this slot's gate is still committed under the
    ///   guard: every reattach of *this* slot targets *this* key, so it is
    ///   serialized all the same.
    pub(super) fn take_slot(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) -> Option<DisposalDebt<T>> {
        match self.inner.slots.entry(tenant.clone()) {
            Entry::Occupied(occupied) => {
                if Arc::ptr_eq(occupied.get(), slot) {
                    // MUST stay inside this critical section — see the rustdoc.
                    let debt = self.commit_dispose(slot);
                    occupied.remove();
                    debt
                } else {
                    // MUST stay inside this critical section — see the rustdoc.
                    self.commit_dispose(slot)
                }
            }
            Entry::Vacant(vacant) => {
                // MUST stay inside this critical section: the guard is bound so
                // that it outlives the CAS, exactly as in
                // `remember_negative_owned`.
                let debt = self.commit_dispose(slot);
                drop(vacant);
                debt
            }
        }
    }

    /// Take the slot's one-shot disposal gate. `Some` means **this caller** owns
    /// the disposal and owes the value a [`run_committed_dispose`].
    ///
    /// # Invariant: never called outside a shard-lock critical section
    ///
    /// Every call site holds the tenant key's `slots` shard lock — either a
    /// `remove_if` predicate or a bound `Entry` guard — because that lock is the
    /// only thing that orders a commit against a concurrent
    /// [`reattach`](Self::reattach) of the same slot. There are six, and they
    /// are the whole list: [`take_ready`](Self::take_ready) (in its predicate),
    /// [`take_slot`](Self::take_slot) (all three entry branches) and `reattach`
    /// (both `Orphaned` arms). A seventh, anywhere outside a guard, reopens the
    /// window this split exists to close. The tests cannot catch a CAS moved to
    /// just *after* its lock — nothing schedulable separates the two — so the
    /// comment at each site is the guard rail.
    ///
    /// The two halves must also stay paired: every `Some` reaches the await
    /// exactly once, or a value is gated shut and never closed. The
    /// [`DisposalDebt`] is what pairs them — it is minted here, inside the
    /// critical section, so `drain` cannot observe the slot leave the map before
    /// the work is counted, and it is only discharged by being dropped.
    ///
    /// Slots with nothing in them (an initialization that never completed)
    /// report `None`: there is nothing to hand to a source, and pretending
    /// otherwise would leave a caller owing an await that cannot do anything.
    pub(super) fn commit_dispose(&self, slot: &Slot<T>) -> Option<DisposalDebt<T>> {
        if slot.cell.get().is_none() {
            return None;
        }
        slot.disposed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DisposalDebt::new(&self.inner))
    }

    /// Run the disposal this caller already won with [`commit_dispose`].
    ///
    /// Takes the debt by value and drops it on the way out, so the work stops
    /// being counted exactly when it is done — including when `dispose` panics
    /// or this future is dropped mid-await.
    pub(super) async fn run_committed_dispose(&self, tenant: &TenantId, slot: &Slot<T>, debt: DisposalDebt<T>) {
        let _debt = debt;
        let Some(value) = slot.cell.get() else {
            return;
        };
        self.inner.counters.disposed.fetch_add(1, Ordering::Relaxed);
        self.inner.wiring.source.dispose(tenant, value.clone()).await;
    }
}

/// One unit of work that keeps a value alive *outside* the map, counted in
/// [`Inner::in_flight`] for as long as the guard exists.
///
/// Two things produce one, and between them they cover every way a live value
/// can be unreachable from the map while still needing to be closed:
///
/// - **A committed disposal.** [`Tenanted::commit_dispose`] mints one the
///   instant its CAS wins — *inside* the shard-lock critical section that took
///   the gate, so there is no window where the gate is committed but the work is
///   uncounted. The owner drops it when the `dispose` await returns.
/// - **A resolve holding a slot.** [`Tenanted::resolve`] mints one before it
///   touches the map and drops it once the slot is classified. This is the half
///   a disposal counter alone cannot cover: an initializer that was cancelled
///   detaches its *empty* slot, a waiter inherits the cell and fills it, and for
///   the whole stretch between that and its `reattach` there is a live value in
///   nobody's map and no gate committed anywhere.
///
/// `Drop` is the only way to discharge it, so a panicking `dispose`, a dropped
/// future and a failed spawn all decrement. Dropping one without doing the work
/// is therefore safe (it degrades to "the value is not closed", the pre-existing
/// no-runtime behaviour) — it can never wedge [`Tenanted::drain`].
pub(super) struct Pending<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Pending<T> {
    pub(super) fn new(inner: &Arc<Inner<T>>) -> Self {
        inner.in_flight.fetch_add(1, Ordering::SeqCst);
        Self {
            inner: Arc::clone(inner),
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if self.inner.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.settled.notify_waiters();
        }
    }
}

/// The obligation to await one committed disposal — a [`Pending`] that a
/// specific caller won and must discharge.
pub(super) type DisposalDebt<T> = Pending<T>;

/// A slot taken out of the map, and the disposal gate that was committed in the
/// same critical section as its removal.
///
/// `debt` is `None` only when someone else had already committed the gate — see
/// [`Tenanted::take_ready`]. The remover then removes and stands down: it cannot
/// await a disposal it does not own.
pub(super) struct Removed<T> {
    pub(super) slot: Arc<Slot<T>>,
    pub(super) debt: Option<DisposalDebt<T>>,
}

/// Spawn a detached task, reporting whether a runtime was available.
pub(super) fn spawn_detached<F>(future: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if !r2e_core::rt::in_runtime() {
        return false;
    }
    // Dropping the handle detaches the task, which is the point: the caller is
    // a synchronous path (`Drop`, `invalidate`) that cannot await disposal.
    drop(r2e_core::rt::spawn(future));
    true
}
