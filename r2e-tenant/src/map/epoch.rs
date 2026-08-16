//! The removal epoch and the millisecond time base the slots are stamped
//! with.

use std::sync::atomic::Ordering;

use super::state::{Slot, Tenanted};

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub(super) fn now_millis(&self) -> u64 {
        self.inner.started.elapsed().as_millis() as u64
    }

    /// The current removal epoch.
    pub(super) fn epoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::SeqCst)
    }

    /// Announce a removal, so creations that started before it stop writing back.
    ///
    /// # Why this is correct, on the key being removed
    ///
    /// The bump happens **before** the removal takes the key's shard lock, and
    /// every writeback reads the epoch *while holding that same lock*. The shard
    /// lock is therefore what orders the two, and there are only two cases:
    ///
    /// - the writeback holds the lock **first**. It may still read the old epoch
    ///   and reattach — and then `take_ready` acquires the lock, finds a slot
    ///   that is now ready, and removes and disposes of it. The removal wins.
    /// - the removal holds the lock **first**. The writeback acquires it
    ///   afterwards, and its read is ordered after the bump by the lock handoff,
    ///   so it sees the new epoch and fences itself.
    ///
    /// Either way the caller of `invalidate`/`evict` gets what it asked for. The
    /// counter is `SeqCst` on both sides so that this argument does not have to
    /// lean on any subtler ordering: a removal path is nowhere near hot enough
    /// for the difference to matter.
    ///
    /// # What does *not* bump
    ///
    /// Only the public removals do. The cleanup of a cancelled or panicking
    /// initializer's **empty** slot removes without bumping, on purpose: that
    /// slot's cell is what a waiter inherits, and fencing it would make the
    /// self-heal in [`reattach`](Self::reattach) impossible. The vacancy it
    /// leaves is covered instead by the disposal gate, committed under the key's
    /// shard guard — see `reattach`.
    ///
    /// # Cross-key bumps
    ///
    /// The counter is **map-wide**, not per key, so a removal on one tenant also
    /// fences detached creations for unrelated ones. That direction needs no
    /// ordering argument: over-fencing only ever disposes of a value and rebuilds
    /// it on the next request. Bumping without removing anything (`invalidate`
    /// on a tenant with nothing ready still bumps, because the bump precedes the
    /// lookup) is the same trade — a false fence costs one rebuild, a missed
    /// fence costs correctness.
    pub(super) fn bump_epoch(&self) {
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn touch(&self, slot: &Slot<T>) {
        slot.last_used.store(self.now_millis(), Ordering::Relaxed);
    }
}
