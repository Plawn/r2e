//! The single-flight create path: `resolve`, the reattach that decides who
//! owns a completed creation, and the negative/fallback verdicts.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use dashmap::mapref::entry::Entry;
use r2e_core::rt;

use crate::error::{BoxError, TenantError};
use crate::source::{ResolutionChain, TenantContext};
use crate::TenantId;

use super::dispose::{DisposalDebt, Pending};
use super::drain::draining_error;
use super::state::{Inner, Slot, Tenanted, Wiring};
use super::TenantedSettings;

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    // ── internals ───────────────────────────────────────────────────────────

    pub(crate) async fn resolve(
        &self,
        tenant: &TenantId,
        chain: ResolutionChain,
    ) -> Result<T, TenantError> {
        let wiring = &self.inner.wiring;

        // Admission is **double-checked**, and the order of the three steps is
        // the whole point. This first read MUST stay ahead of `Pending::new`:
        // moving it after (or dropping it, since the second read looks
        // redundant) reintroduces the starvation below, and no test can see the
        // difference — the rejected path mints and drops its guard without an
        // await in between, so the counter is back down before `resolve`
        // returns and the damage is only ever visible as a livelock under
        // sustained overlap. The structure carries this one.
        //
        // The first read touches no shared counter. Every request that arrives
        // after the latch store fails it and leaves without ever incrementing —
        // which is what keeps a flood of post-shutdown 503s from holding the
        // counter permanently above zero and starving `drain` forever. (Under
        // the plugin lifecycle the listener is still accepting while this hook
        // runs, so that flood is an ordinary production shape, not a contrived
        // one.)
        if self.is_draining() {
            return Err(draining_error(tenant));
        }

        // Only now is the work counted, and only a resolve that passes the
        // *second* read is admitted: it is dropped again immediately otherwise.
        // In the `SeqCst` total order an admitted resolve has
        //
        //     increment  <  re-check  <  latch store  <  drain's counter read
        //
        // (the re-check read `false`, and the latch only ever goes `false` ->
        // `true`, so a load that reads `false` precedes every `true` store in
        // the total order — including this `drain`'s, which in turn precedes
        // its own counter read by program order; and the increment precedes the
        // re-check by program order). So `drain` either sees this increment and
        // waits for it, or this resolve had already decremented — in which case
        // it finished classifying its slot before the read, and the pass that
        // follows sees whatever it left mapped. Never both missing.
        //
        // The transient incrementers are therefore exactly the finite set that
        // was already past the first read when the latch went up: the counter
        // strictly drains, which is what makes `drain`'s notify-on-zero wake up.
        let _in_flight = Pending::new(&self.inner);
        if self.is_draining() {
            return Err(draining_error(tenant));
        }

        // Before the negative cache, not after: a live resource always beats a
        // negative memo. The two can coexist — a creation that finished into a
        // *detached* cell can report the tenant unknown after another slot
        // cached a real value under the same key — and honouring the memo there
        // would shadow a working tenant (404 or fallback) for a whole
        // `negative-ttl`.
        if let Some(value) = self.hit(tenant) {
            return Ok(value);
        }

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
                // The guard lives *inside* the initializer, so only the task
                // that actually runs a creation arms it. A waiter parked on the
                // cell never enters this closure: cancelling it must not touch
                // the map, or it would detach the slot of the creation it was
                // waiting for (whose value would then never be disposed of).
                //
                // What it covers is the two paths that leave the initializer
                // without returning a value — a panic inside
                // `TenantSource::create`, and this future being dropped
                // mid-creation (a client disconnect, an outer timeout) — either
                // of which would otherwise leave an empty slot mapped with
                // nobody to retry it, one leaked entry per hostile tenant id.
                // The error path disarms it and lets `resolve` below own the
                // removal.
                let mut cleanup = EmptySlotGuard {
                    inner: &self.inner,
                    tenant,
                    slot: &slot,
                    armed: true,
                };
                // Stamp the slot with the epoch *this* initialization starts at,
                // before the source is asked. Two writes can land here: the
                // resolver that created the slot wrote its own reading in
                // `slot_for`, and whoever ends up running the initializer (this
                // task — possibly a waiter that inherited the cell after a
                // failure, seconds later) overwrites it now. This one wins,
                // because it is the one that actually brackets the `create`
                // call; the earlier write is only ever *older*, which would
                // over-fence rather than under-fence, so losing the race in the
                // other direction would be safe too. Initializers never run
                // concurrently on one cell, so there is no third case.
                slot.epoch.store(self.epoch(), Ordering::SeqCst);
                let outcome = async {
                    // First thing inside the initializer, not just before it:
                    // the waiters that queued behind a failed init run this
                    // closure in turn, and this is where they see the negative
                    // entry the first attempt wrote. Without it, an
                    // unknown-tenant wave calls the directory once per waiter.
                    if self.negative_hit(tenant, &wiring.settings) {
                        return Err(CreateFailure::Unknown);
                    }
                    let ctx = TenantContext::new(tenant, wiring.graph.clone(), chain);
                    let creating = wiring.source.create(tenant, &ctx);
                    let created = match wiring.settings.create_timeout {
                        Some(budget) => match rt::timeout(budget, creating).await {
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
                            self.remember_negative_owned(tenant, &slot, &wiring.settings);
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
                }
                .await;
                // Both outcomes are handled by `resolve`: `Ok` by the reattach
                // below, `Err` by the removal below. One owner per path.
                cleanup.armed = false;
                outcome
            })
            .await;

        match outcome {
            Ok(value) => {
                let value = value.clone();
                if self.is_draining() {
                    // Shutdown started while this creation was in flight. The
                    // value is real and cached, so it has to be disposed of
                    // rather than handed out: `drain` may already have walked
                    // past this slot. The per-slot gate keeps that from being a
                    // double dispose if it has not.
                    self.detach_and_dispose(tenant, &slot).await;
                    return Err(draining_error(tenant));
                }

                // Self-heal. This cell may have been detached from the map
                // while the creation ran: an earlier attempt on it was
                // cancelled (its guard removed the slot) or failed (this
                // `resolve`'s `Err` arm removed it), and this task inherited the
                // cell as a waiter. Putting the slot back is what makes the
                // value the map's again — and therefore disposable.
                let ownership = self.reattach(tenant, &slot);
                if !matches!(ownership, SlotOwnership::Kept) {
                    // The initializer clears the negative entry on success, but
                    // a *racing* attempt for the same tenant may have written
                    // one after that. A success wins.
                    self.inner.negative.remove(tenant);
                }
                // A concurrent resolve recreated the key while this creation ran
                // (or a removal fenced this one off), and this value belongs to
                // nobody. `reattach` already committed the slot's gate under the
                // shard lock; a `Some` debt says it was *this* caller that won
                // it, in which case only the await is left to spawn (`None`:
                // somebody else owns it, spawn nothing). The value is still
                // handed out — `get` never handed out a lease, so
                // close-while-cloned is already part of the contract.
                //
                // This runs before the drain re-check below on purpose. The gate
                // is already taken, so `detach_and_dispose` there would find it
                // committed and await nothing — the debt has to be discharged
                // first.
                if let SlotOwnership::Orphaned { debt: Some(debt) } = ownership {
                    self.spawn_committed_dispose(tenant, &slot, debt);
                }
                if self.is_draining() {
                    // The latch went up between the check above and the
                    // reattach: undo it, or a slot put back behind `drain`'s
                    // last pass would stay mapped and never be disposed of.
                    self.detach_and_dispose(tenant, &slot).await;
                    return Err(draining_error(tenant));
                }
                self.touch(&slot);
                self.enforce_max_active(&wiring.settings);
                Ok(value)
            }
            Err(failure) => {
                // Nothing is cached on failure, and this arm is the only owner
                // of that removal (the guard disarmed itself). Two conditions:
                // `ptr_eq` keeps a concurrent retry's fresh slot from being
                // removed by this one's cleanup, and `is_ready` keeps a waiter
                // that already succeeded *on this very cell* from having its
                // value detached — both are evaluated under the shard lock.
                self.inner.slots.remove_if(tenant, |_, current| {
                    Arc::ptr_eq(current, &slot) && !current.is_ready()
                });
                match failure {
                    CreateFailure::Failed(err) => Err(err),
                    CreateFailure::Unknown => self.unknown_or_fallback(tenant, wiring),
                }
            }
        }
    }

    /// Put `slot` back under `tenant` after a creation that may have been
    /// detached, reporting who owns the key.
    ///
    /// The whole decision happens under one shard lock, so a concurrent
    /// `slot_for` either loses the key to this slot or is already the occupant.
    ///
    /// Filling a vacant key takes two things beyond the lock, because a vacant
    /// key reads the same whether nobody ever mapped this tenant or an
    /// [`invalidate`](Self::invalidate) just emptied it:
    ///
    /// - **the epoch has not moved** since this *initialization* started
    ///   (`slot.epoch()` — one reading shared by every participant on this cell,
    ///   rather than a per-caller capture). See [`bump_epoch`](Self::bump_epoch)
    ///   for why the shard lock makes that reading trustworthy.
    /// - **the disposal gate is untaken.** A slot whose gate is committed holds
    ///   a value that is closed or closing; putting it back would cache a dead
    ///   resource.
    ///
    /// # Why the gate is committed *here*, inline
    ///
    /// Two participants can share one cell and still classify its value
    /// differently — not because they read different epochs (they read one, off
    /// the slot), but because the *key* changes underneath them. A competing
    /// empty slot appears (one participant orphans against it) and then vanishes
    /// when its own initializer fails or is cancelled — and that cleanup
    /// deliberately does **not** bump the epoch, or the legitimate
    /// waiter-inherits-and-retries reattach could never happen. The next
    /// participant then finds a vacant key at a matching epoch: exactly the
    /// state that says "restore".
    ///
    /// So orphaning does not merely *schedule* a disposal, it **commits the
    /// slot's one-shot gate right here, under the entry guard**, and reports
    /// whether it won it. That puts the commit and the restore decision in the
    /// same critical section, and the shard lock does the rest:
    ///
    /// | Order under the key's shard lock | What the other participant sees | Outcome |
    /// |---|---|---|
    /// | orphan first | vacant + matching epoch, but `is_disposed()` — read under the same lock | refuses, `Orphaned` with the gate already lost: it spawns nothing |
    /// | restore first | the key occupied by **this very slot** | `ptr_eq` → `Kept`: no gate commit, no disposal |
    ///
    /// Either way exactly one participant owns the disposal, and a disposed
    /// value is never the map's. Committing inside the spawned disposal task
    /// instead would leave the window open for the whole scheduling delay.
    ///
    /// # And against a concurrent public removal
    ///
    /// | Removal reaches the shard lock | This restore reads | Outcome |
    /// |---|---|---|
    /// | after this restore | old epoch → inserted | the removal's `take_ready` then finds a **ready** slot under the key and removes and disposes of it: the removal still wins |
    /// | before this restore | bumped epoch (lock handoff orders the read after the bump) | `Orphaned`, and the CAS is **lost**: `take_ready` committed the gate inside the very critical section that removed the slot, so this caller spawns nothing and the remover's own `await` still closes the value |
    /// | not at all (unrelated key) | possibly bumped anyway | `Orphaned` at worst — a false fence costs one rebuild |
    ///
    /// The `debt` the `Orphaned` arms carry is the gate's verdict:
    /// `true` means this caller — and nobody else — must reach
    /// [`run_committed_dispose`](Self::run_committed_dispose).
    pub(super) fn reattach(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) -> SlotOwnership<T> {
        match self.inner.slots.entry(tenant.clone()) {
            Entry::Vacant(vacant) => {
                if self.epoch() == slot.epoch() && !slot.is_disposed() {
                    vacant.insert(Arc::clone(slot));
                    return SlotOwnership::Restored;
                }
                // Stale by construction: something was removed while this
                // creation ran (or the value is already being closed) and this
                // key is empty. Orphan it — disposed of, still handed to its
                // caller. The `commit_dispose` also *closes* the window above
                // for whoever comes next, and MUST stay inside this entry guard
                // to do so.
                SlotOwnership::Orphaned {
                    debt: self.commit_dispose(slot),
                }
            }
            Entry::Occupied(occupied) => {
                if Arc::ptr_eq(occupied.get(), slot) {
                    SlotOwnership::Kept
                } else {
                    // MUST stay inside this entry guard — see `commit_dispose`.
                    SlotOwnership::Orphaned {
                        debt: self.commit_dispose(slot),
                    }
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

    /// The mapped, **ready** value for `tenant`, counted as a cache hit.
    ///
    /// [`resolve`](Self::resolve) runs this ahead of the negative cache, so a
    /// value the map actually holds can never be shadowed by a stale "unknown"
    /// memo — and the memo is dropped on the way out, so the next request does
    /// not pay for this check either.
    fn hit(&self, tenant: &TenantId) -> Option<T> {
        let slot = self
            .inner
            .slots
            .get(tenant)
            .map(|entry| Arc::clone(entry.value()))?;
        let value = slot.cell.get()?.clone();
        self.touch(&slot);
        self.inner.counters.hits.fetch_add(1, Ordering::Relaxed);
        // `contains_key` first: a read lock on the shard, where the `remove`
        // this almost always skips would take a write lock per cache hit.
        if self.inner.negative.contains_key(tenant) {
            self.inner.negative.remove(tenant);
        }
        Some(value)
    }

    /// Remember `tenant` as unknown — but only on the word of an attempt that
    /// still speaks for the key.
    ///
    /// A *detached* attempt speaks for nobody: another slot may already hold a
    /// real value for this tenant, and remembering "unknown" over it would
    /// shadow a working tenant for a whole `negative-ttl`. A vacant key is only
    /// this attempt's own cleanup when nothing was removed since it started —
    /// hence the same epoch fence as [`reattach`](Self::reattach), so an
    /// `Ok(None)` from before an `invalidate` cannot repopulate the negative
    /// cache that `invalidate` just cleared.
    ///
    /// The test and the insert are one critical section, under the `slots` shard
    /// lock: split, a fresh resolve could install its slot in between and then
    /// be aborted by this entry at the negative re-check *inside* its own
    /// initializer, without the source ever being asked.
    ///
    /// What the fence deliberately does **not** cover: a creation that is still
    /// *mapped* (the occupied branch) writes its memo even if an `invalidate`
    /// ran while it was in flight. Removal only ever touches ready slots, so
    /// that creation was never removed; it **overlaps** the invalidation and is
    /// deliberately not fenced. Only its completion is concurrent — the answer
    /// the source gave it may well predate the invalidation — but a request
    /// arriving right after the invalidation would have asked the source at the
    /// same moment and got the same thing, so there is nothing better to do.
    ///
    /// # Lock order
    ///
    /// This is the only place that touches `negative` while holding a `slots`
    /// guard, and it fixes the order **`slots` → `negative`** for the whole
    /// file. No path may take a `slots` guard while holding a `negative` one.
    /// Bounding the cache is deliberately left outside the guard: it walks every
    /// negative shard and needs none of this atomicity.
    pub(super) fn remember_negative_owned(
        &self,
        tenant: &TenantId,
        slot: &Arc<Slot<T>>,
        settings: &TenantedSettings,
    ) {
        if settings.negative_ttl.is_none() || settings.max_negative == 0 {
            return;
        }
        let now = self.now_millis();
        {
            // `entry` is *bound*, not matched-and-dropped: the guard has to
            // outlive the insert below, or the ownership test and the write are
            // two separate critical sections again and everything between them
            // (a fresh resolve installing its slot, a removal bumping the epoch)
            // slips into the gap.
            let entry = self.inner.slots.entry(tenant.clone());
            let owned = match &entry {
                Entry::Occupied(occupied) => Arc::ptr_eq(occupied.get(), slot),
                Entry::Vacant(_) => self.epoch() == slot.epoch(),
            };
            if owned {
                self.inner.negative.insert(tenant.clone(), now);
            }
            drop(entry);
            if !owned {
                return;
            }
        }
        self.bound_negative(tenant, settings);
    }

    pub(super) fn slot_for(&self, tenant: &TenantId) -> Arc<Slot<T>> {
        // Clone the `Arc` out and drop the guard before returning: every caller
        // awaits, and awaiting under a DashMap guard deadlocks the shard.
        if let Some(existing) = self.inner.slots.get(tenant) {
            return Arc::clone(existing.value());
        }
        let now = self.now_millis();
        let epoch = self.epoch();
        Arc::clone(
            self.inner
                .slots
                .entry(tenant.clone())
                .or_insert_with(|| Arc::new(Slot::new(now, epoch)))
                .value(),
        )
    }
}

/// Outcome of a failed `create`, kept internal so the `OnceCell` initializer can
/// distinguish "unknown tenant" (which may still fall back) from a real error.
enum CreateFailure {
    Failed(TenantError),
    Unknown,
}

/// Who owns a tenant's key once a creation completes.
pub(super) enum SlotOwnership<T> {
    /// The map still holds this slot — the ordinary case, nothing to heal.
    Kept,
    /// The slot had been detached (a cancelled or failed earlier attempt on the
    /// same cell) and was put back: the value is the map's again.
    Restored,
    /// A concurrent resolve recreated the key with a different slot, or a
    /// removal fenced this attempt off. This value is not the map's and has to
    /// be disposed of.
    ///
    /// The slot's one-shot gate is **already committed** when this is returned —
    /// [`Tenanted::reattach`] takes it under the key's shard guard, which is what
    /// keeps a later participant from restoring a dying value. `debt` is that
    /// CAS's verdict: `Some` means this caller won it and must reach
    /// `run_committed_dispose`; `None` means someone else owns the await and this
    /// caller must **not** spawn one.
    Orphaned { debt: Option<DisposalDebt<T>> },
}

/// Removes an empty slot if the initialization it guards never returns.
///
/// Armed **inside** the `OnceCell` initializer, so only the task actually
/// running a creation carries it — a waiter parked on the cell has no guard and
/// its cancellation leaves the map alone. The `Ok` and `Err` paths of `resolve`
/// disarm it and own the map surgery themselves; what is left for the guard is
/// the two paths that return through neither — a panic inside
/// `TenantSource::create`, and the initializing future being dropped mid-create
/// (a client disconnect, a `rt::timeout` around the handler). Either
/// would leave an empty slot mapped with no waiter to retry it, which a hostile
/// tenant id could farm.
///
/// The slot it detaches is not lost: a waiter that inherits the cell and
/// succeeds reattaches it (see [`Tenanted::reattach`]).
struct EmptySlotGuard<'a, T> {
    inner: &'a Inner<T>,
    tenant: &'a TenantId,
    slot: &'a Arc<Slot<T>>,
    armed: bool,
}

impl<T> Drop for EmptySlotGuard<'_, T> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Both conditions under the shard lock: `ptr_eq` never removes the
        // replacement slot a retry installed, `is_ready` never detaches a value
        // that landed in this very slot meanwhile (it would never be disposed
        // of).
        self.inner.slots.remove_if(self.tenant, |_, current| {
            Arc::ptr_eq(current, self.slot) && !current.is_ready()
        });
    }
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
