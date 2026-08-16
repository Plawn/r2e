//! `#[doc(hidden)]` seams that replay the await-free races no concurrent
//! test can reach. Not API.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::TenantId;

use super::resolve::SlotOwnership;
use super::state::Tenanted;

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Write a negative-cache entry for `tenant`, bypassing every ownership and
    /// epoch rule.
    ///
    /// A test seam, not API. "A ready slot and a fresh negative entry for the
    /// same tenant" is precisely the state
    /// [`remember_negative_owned`](Self::remember_negative_owned) exists to
    /// prevent, so the test that pins the *other* half of the defence —
    /// `resolve` consulting the ready slot before the memo, which is what keeps
    /// a live tenant up if that first rule ever regresses — has no honest way to
    /// reach it. Rather than let the two rules mask each other, this reaches in.
    #[doc(hidden)]
    pub fn force_negative_entry(&self, tenant: &TenantId) {
        self.inner.negative.insert(tenant.clone(), self.now_millis());
    }

    /// A test seam, not API. Removes and disposes of `tenant`'s slot and then
    /// asks [`reattach`](Self::reattach) to put that same slot back, with the
    /// epoch rule deliberately neutralised (the slot is re-stamped with the
    /// *current* epoch) so that only the disposal gate can refuse it.
    ///
    /// Returns `true` when the reattach was refused.
    #[doc(hidden)]
    pub async fn force_reattach_after_dispose(&self, tenant: &TenantId) -> bool {
        let Some(removed) = self.take_ready(tenant) else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.run_committed_dispose(tenant, &removed.slot, debt).await;
        }
        removed.slot.epoch.store(self.epoch(), Ordering::SeqCst);
        matches!(
            self.reattach(tenant, &removed.slot),
            SlotOwnership::Orphaned { .. }
        )
    }

    /// A test seam, not API. Replays the remover-against-a-late-participant race
    /// in a chosen order.
    ///
    /// The participant is one that got its value out of `get_or_try_init` just
    /// as a removal ran, and reaches [`reattach`](Self::reattach) either side of
    /// it. The remover half is what [`evict`](Self::evict) does — `take_ready`
    /// (bump, remove and commit the gate under the key's shard lock) then the
    /// `dispose` await — spelled out here so the participant can be spliced into
    /// the window between the two, which no concurrent test can reach: it is
    /// await-free.
    ///
    /// - `remove_first`: returns `(true, true)` when the remover owned the
    ///   disposal and the late participant **lost** the gate, spawning nothing —
    ///   i.e. the value was really closed by the time the remover returned.
    /// - `!remove_first`: the participant restores the slot first; returns
    ///   `(true, true)` when it restored and the remover then removed *and*
    ///   owned the disposal of the re-mapped slot.
    #[doc(hidden)]
    pub async fn force_remove_race(&self, tenant: &TenantId, remove_first: bool) -> (bool, bool) {
        if remove_first {
            // The participant's handle, taken while the slot is still the map's:
            // exactly what an initialization that returned a moment too early
            // is holding.
            let Some(shared) = self.inner.slots.get(tenant).map(|e| Arc::clone(e.value())) else {
                return (false, false);
            };
            let Some(removed) = self.take_ready(tenant) else {
                return (false, false);
            };
            let late = self.reattach(tenant, &shared);
            let owed = removed.debt.is_some();
            if let Some(debt) = removed.debt {
                self.run_committed_dispose(tenant, &removed.slot, debt).await;
            }
            (
                owed,
                matches!(late, SlotOwnership::Orphaned { debt: None }),
            )
        } else {
            // Detach the ready slot *without* taking its gate — the state a
            // participant holding a live, unmapped slot is in.
            self.bump_epoch();
            let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready())
            else {
                return (false, false);
            };
            shared.epoch.store(self.epoch(), Ordering::SeqCst);
            let restored = matches!(self.reattach(tenant, &shared), SlotOwnership::Restored);
            let owed = match self.take_ready(tenant) {
                Some(removed) => match removed.debt {
                    Some(debt) => {
                        self.run_committed_dispose(tenant, &removed.slot, debt).await;
                        true
                    }
                    None => false,
                },
                None => false,
            };
            (restored, owed)
        }
    }

    /// A test seam, not API. Replays the drain-against-a-late-participant race
    /// on a **detached** slot, in a chosen order.
    ///
    /// The shape is the one that made the old lock-free
    /// [`take_slot`](Self::take_slot) fallback unsound: a participant `P` holds a
    /// live slot that is no longer under the key (its initializer's competitor
    /// vanished, or it was detached), while a drain-side caller `A` reaches
    /// `take_slot` for that same slot. `A` therefore takes the branch where the
    /// key does **not** hold its slot — vacant, or holding somebody else's. The
    /// slot is re-stamped with the current epoch so the epoch rule cannot answer
    /// for the gate rule; only the under-guard commit can.
    ///
    /// - `!restore_first`: `A` runs first and finds the key **vacant** — the
    ///   fallback shape. Returns `(true, true)` when `A` owed the disposal and
    ///   `P`'s later `reattach` refused to restore, spawning nothing.
    /// - `restore_first`: `P` restores the slot first, so `A` finds the key
    ///   **occupied by that very slot**. Returns `(true, true)` when `A` owed the
    ///   disposal and the key is empty afterwards.
    ///
    /// Either way the debt `A` takes on is awaited before this returns, exactly
    /// as `drain` and `detach_and_dispose` await theirs.
    #[doc(hidden)]
    pub async fn force_take_slot_race(&self, tenant: &TenantId, restore_first: bool) -> (bool, bool) {
        // Detach the ready slot *without* taking its gate: a live value held by
        // a participant, unmapped.
        self.bump_epoch();
        let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready()) else {
            return (false, false);
        };
        shared.epoch.store(self.epoch(), Ordering::SeqCst);

        if restore_first {
            let restored = matches!(self.reattach(tenant, &shared), SlotOwnership::Restored);
            let debt = self.take_slot(tenant, &shared);
            let owed = debt.is_some();
            if let Some(debt) = debt {
                self.run_committed_dispose(tenant, &shared, debt).await;
            }
            (owed, restored && self.inner.slots.get(tenant).is_none())
        } else {
            let debt = self.take_slot(tenant, &shared);
            let owed = debt.is_some();
            let late = self.reattach(tenant, &shared);
            if let Some(debt) = debt {
                self.run_committed_dispose(tenant, &shared, debt).await;
            }
            (owed, matches!(late, SlotOwnership::Orphaned { debt: None }))
        }
    }

    /// A test seam, not API. Replays the two-participant race
    /// [`reattach`](Self::reattach) exists to close, in a chosen order.
    ///
    /// The shape: one ready slot `S` shared by two participants, a competing
    /// empty slot `S2` installed under the key, and `S2` disappearing when its
    /// own initializer fails — a cleanup that deliberately does not bump the
    /// epoch. `S` is re-stamped with the current epoch so that the epoch rule
    /// cannot answer for the gate rule; only the under-lock gate commit can.
    ///
    /// - `orphan_first`: participant 1 classifies against `S2` (→ `Orphaned`,
    ///   gate committed inline), `S2` vanishes, participant 2 then finds a
    ///   vacant key at a matching epoch. Returns `(true, true)` when P1 owed the
    ///   disposal and P2 **refused** to restore.
    /// - `!orphan_first`: `S2` vanishes first, participant 2 restores `S`, and
    ///   participant 1 then finds the key holding `S` itself. Returns
    ///   `(true, true)` when P2 restored and P1 answered `Kept` — spawning
    ///   nothing.
    ///
    /// Whatever the order, the disposal debt this seam creates is discharged
    /// before it returns, so a test can assert on `disposals()` directly.
    #[doc(hidden)]
    pub async fn force_reattach_race(&self, tenant: &TenantId, orphan_first: bool) -> (bool, bool) {
        // Detach the ready slot *without* taking its gate: this seam is about
        // two participants sharing a live value, not about a removal.
        self.bump_epoch();
        let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready())
        else {
            return (false, false);
        };
        // The competing empty slot, still initializing as far as the map knows.
        let competitor = self.slot_for(tenant);
        shared.epoch.store(self.epoch(), Ordering::SeqCst);

        // The competitor's initializer failed or was cancelled: its cleanup
        // removes the empty slot and, by design, bumps nothing.
        let drop_competitor = || {
            self.inner.slots.remove_if(tenant, |_, current| {
                Arc::ptr_eq(current, &competitor) && !current.is_ready()
            });
        };

        if orphan_first {
            let p1 = self.reattach(tenant, &shared);
            drop_competitor();
            let p2 = self.reattach(tenant, &shared);
            let owed = match p1 {
                SlotOwnership::Orphaned { debt: Some(debt) } => {
                    // Discharge the debt P1 took on, so the test sees a settled
                    // world rather than a value gated shut and never closed.
                    self.run_committed_dispose(tenant, &shared, debt).await;
                    true
                }
                _ => false,
            };
            (owed, !matches!(p2, SlotOwnership::Restored))
        } else {
            drop_competitor();
            let p2 = self.reattach(tenant, &shared);
            let p1 = self.reattach(tenant, &shared);
            // Nothing was committed here — `Kept` takes no gate — so nothing is
            // awaited: the slot is back in the map, alive.
            (
                matches!(p2, SlotOwnership::Restored),
                matches!(p1, SlotOwnership::Kept),
            )
        }
    }
}
