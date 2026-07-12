//! Per-partition offset tracking for at-least-once commit ordering.
//!
//! Kafka handlers run concurrently, so messages complete out of order, but a
//! Kafka commit is a single low-watermark per partition: storing offset N
//! asserts every offset `<= N` on that partition is processed. This tracker
//! only allows storing an offset once every *received* offset at or below it
//! has been acked. It deliberately does not assume partition offsets are
//! contiguous — compacted topics and transactional control markers leave
//! legitimate gaps in the offset sequence, and a contiguity assumption would
//! stall the watermark forever at the first gap.

use std::collections::{BTreeSet, HashMap};

/// Tracks per-partition commit watermarks across out-of-order handler
/// completions.
#[derive(Default)]
pub struct OffsetTracker {
    partitions: HashMap<i32, PartitionState>,
}

#[derive(Default)]
struct PartitionState {
    /// Offsets received but not yet acked. Nacked offsets stay here forever,
    /// pinning the commit boundary at/below them.
    in_flight: BTreeSet<i64>,
    /// Acked offsets not yet committable because a lower offset is in flight.
    pending_acked: BTreeSet<i64>,
    /// Highest offset already reported for storing (guards against regressing
    /// the stored watermark when a reassigned partition redelivers).
    stored: Option<i64>,
}

impl OffsetTracker {
    /// Create an empty tracker (fresh per consumer lifetime).
    pub fn new() -> Self {
        Self { partitions: HashMap::new() }
    }

    /// Record that a message was received.
    ///
    /// Must be called before [`on_ack`](Self::on_ack) for the same offset:
    /// the set of received in-flight offsets is what gates the watermark.
    pub fn on_receive(&mut self, partition: i32, offset: i64) {
        self.partitions.entry(partition).or_default().in_flight.insert(offset);
    }

    /// Record an Ack completion for `(partition, offset)`.
    ///
    /// Returns `Some(store_offset)` when the watermark advanced — the caller
    /// should `store_offset(topic, partition, store_offset)` (the offset of
    /// the highest now-committable message; librdkafka commits `+1`
    /// internally). Returns `None` when a lower received offset is still in
    /// flight (or nacked), or the offset was not in flight (stale duplicate).
    pub fn on_ack(&mut self, partition: i32, offset: i64) -> Option<i64> {
        let st = self.partitions.get_mut(&partition)?;
        if !st.in_flight.remove(&offset) {
            return None;
        }
        st.pending_acked.insert(offset);

        // Committable = acked offsets strictly below the lowest offset still
        // in flight (nacked offsets never leave `in_flight`, so nothing at or
        // above a nack is ever stored).
        let boundary = st.in_flight.first().copied().unwrap_or(i64::MAX);
        let highest = st.pending_acked.range(..boundary).next_back().copied()?;
        st.pending_acked = st.pending_acked.split_off(&(highest + 1));

        if st.stored.is_some_and(|s| highest <= s) {
            return None;
        }
        st.stored = Some(highest);
        Some(highest)
    }

    /// Record a Nack completion for `(partition, offset)`.
    ///
    /// Intentionally a no-op: the offset stays in the in-flight set, so the
    /// commit boundary can never pass it — higher acked offsets stay unstored
    /// until a fresh consumer resumes from the last committed offset
    /// (redelivery on rebalance/restart/reconnect). No seek-based immediate
    /// redelivery is attempted.
    pub fn on_nack(&mut self, _partition: i32, _offset: i64) {}
}
