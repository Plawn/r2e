//! Per-partition commit-watermark tracking for at-least-once backends.
//!
//! Handlers run concurrently, so messages complete out of order, but a broker
//! commit is a single low-watermark per partition: committing offset N asserts
//! every offset `<= N` on that partition is processed. This tracker only
//! reports an offset as committable once every *received* offset at or below
//! it has been acked. It does not assume offsets are contiguous — compacted
//! topics and transactional control markers leave legitimate gaps.
//!
//! State is bounded even under sustained nacks: once a partition has a nacked
//! offset, nothing at or above it can ever commit in this tracker's lifetime,
//! so acked offsets above the lowest nack are dropped instead of accumulated,
//! and newly received offsets above it are not tracked at all.
//!
//! The tracker must live for one consumer session. On a partition
//! revoke/reassign (rebalance), call [`WatermarkTracker::remove_partition`] so
//! redelivered offsets are re-tracked from scratch — stale state from the
//! previous assignment would otherwise suppress commits of redelivered
//! messages.

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

/// Tracks per-partition commit watermarks across out-of-order handler
/// completions. Generic over the backend's partition id and offset types
/// (Kafka: `i32`/`i64`, Iggy: `u32`/`u64`).
pub struct WatermarkTracker<P, O> {
    partitions: HashMap<P, PartitionState<O>>,
}

struct PartitionState<O> {
    /// Offsets received and still being handled.
    in_flight: BTreeSet<O>,
    /// Acked offsets not yet committable because a lower offset is in flight.
    /// Only offsets below `min_nacked` are kept (higher ones can never commit).
    pending_acked: BTreeSet<O>,
    /// Highest offset already reported for committing (guards against
    /// regressing the stored watermark on duplicate completions).
    stored: Option<O>,
    /// Lowest offset that nacked. Pins the commit boundary: nothing at or
    /// above it may ever be reported committable for this partition again.
    min_nacked: Option<O>,
}

impl<O: Ord + Copy> Default for PartitionState<O> {
    fn default() -> Self {
        Self {
            in_flight: BTreeSet::new(),
            pending_acked: BTreeSet::new(),
            stored: None,
            min_nacked: None,
        }
    }
}

impl<P: Eq + Hash + Copy, O: Ord + Copy> WatermarkTracker<P, O> {
    /// Create an empty tracker (fresh per consumer session).
    pub fn new() -> Self {
        Self { partitions: HashMap::new() }
    }

    /// Record that a message was received.
    ///
    /// Must be called before [`on_ack`](Self::on_ack) for the same offset:
    /// the set of received in-flight offsets is what gates the watermark.
    /// Offsets above a nacked offset are not tracked (they can never commit).
    pub fn on_receive(&mut self, partition: P, offset: O) {
        let st = self.partitions.entry(partition).or_default();
        if st.min_nacked.is_some_and(|n| offset >= n) {
            return;
        }
        st.in_flight.insert(offset);
    }

    /// Record an Ack completion for `(partition, offset)`.
    ///
    /// Returns `Some(commit_offset)` when the watermark advanced — the offset
    /// of the highest now-committable message. Returns `None` when a lower
    /// received offset is still in flight, the partition is pinned by a nack
    /// at or below this offset, or the offset was not tracked (stale
    /// duplicate / received after a nack).
    pub fn on_ack(&mut self, partition: P, offset: O) -> Option<O> {
        let st = self.partitions.get_mut(&partition)?;
        if !st.in_flight.remove(&offset) {
            return None;
        }
        if st.min_nacked.is_some_and(|n| offset >= n) {
            // Can never commit — don't accumulate it.
            return None;
        }
        st.pending_acked.insert(offset);

        // Committable = acked offsets strictly below both the lowest offset
        // still in flight and the lowest nacked offset.
        let boundary = match (st.in_flight.first().copied(), st.min_nacked) {
            (Some(f), Some(n)) => Some(f.min(n)),
            (Some(f), None) => Some(f),
            (None, n) => n,
        };
        let highest = match boundary {
            Some(b) => st.pending_acked.range(..b).next_back().copied()?,
            None => st.pending_acked.last().copied()?,
        };
        // Drain everything at or below the new watermark.
        st.pending_acked.retain(|&o| o > highest);

        if st.stored.is_some_and(|s| highest <= s) {
            return None;
        }
        st.stored = Some(highest);
        Some(highest)
    }

    /// Record a Nack completion for `(partition, offset)`.
    ///
    /// Pins the partition's commit boundary at this offset: nothing at or
    /// above it is ever reported committable again (redelivery happens on
    /// rebalance/restart). Acked offsets above it are discarded to keep
    /// memory bounded under sustained traffic.
    pub fn on_nack(&mut self, partition: P, offset: O) {
        let st = self.partitions.entry(partition).or_default();
        st.in_flight.remove(&offset);
        if st.min_nacked.is_none_or(|n| offset < n) {
            st.min_nacked = Some(offset);
        }
        let pin = st.min_nacked.expect("just set");
        st.pending_acked.retain(|&o| o < pin);
        // In-flight offsets above the pin will be dropped when they complete;
        // new receives above it are skipped in on_receive.
    }

    /// Forget all state for a partition.
    ///
    /// Call when the partition is revoked in a rebalance: on reassignment the
    /// broker redelivers from the committed offset and tracking must restart
    /// from scratch.
    pub fn remove_partition(&mut self, partition: &P) {
        self.partitions.remove(partition);
    }

    /// Forget all partitions (e.g. on a full assignment reset).
    pub fn clear(&mut self) {
        self.partitions.clear();
    }
}

impl<P: Eq + Hash + Copy, O: Ord + Copy> Default for WatermarkTracker<P, O> {
    fn default() -> Self {
        Self::new()
    }
}
