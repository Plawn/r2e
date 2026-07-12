//! Tests for `backend::watermark` — the shared per-partition commit tracker.

use r2e_events::backend::WatermarkTracker;

const P: i32 = 0;

fn tracker() -> WatermarkTracker<i32, i64> {
    WatermarkTracker::new()
}

#[test]
fn in_order_completions_advance_watermark_each_step() {
    let mut t = tracker();
    for o in 5..=7 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 6), Some(6));
    assert_eq!(t.on_ack(P, 7), Some(7));
}

#[test]
fn out_of_order_completions_commit_only_on_contiguous_prefix() {
    let mut t = tracker();
    for o in 5..=7 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 7), None);
    assert_eq!(t.on_ack(P, 6), None);
    assert_eq!(t.on_ack(P, 5), Some(7));
}

#[test]
fn partial_prefix_advances_to_gap_only() {
    let mut t = tracker();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 8), None);
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 7), None);
    assert_eq!(t.on_ack(P, 6), Some(8));
}

#[test]
fn offset_gaps_do_not_stall_the_watermark() {
    // Compacted topics / transactional markers leave holes in the sequence.
    let mut t = tracker();
    t.on_receive(P, 5);
    t.on_receive(P, 7);
    t.on_receive(P, 9);
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 9), None);
    assert_eq!(t.on_ack(P, 7), Some(9));
}

#[test]
fn nack_pins_the_partition_at_the_nacked_offset() {
    let mut t = tracker();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 5), Some(5));
    t.on_nack(P, 6);
    assert_eq!(t.on_ack(P, 7), None);
    assert_eq!(t.on_ack(P, 8), None);
}

#[test]
fn acks_below_the_nack_still_commit() {
    let mut t = tracker();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    t.on_nack(P, 7);
    // 5 and 6 are below the pin: they remain committable.
    assert_eq!(t.on_ack(P, 6), None); // 5 still in flight
    assert_eq!(t.on_ack(P, 5), Some(6));
    assert_eq!(t.on_ack(P, 8), None); // above the pin, never commits
}

#[test]
fn nack_pin_survives_across_later_receives() {
    // The cross-batch loss bug: the pin must persist for the tracker lifetime,
    // not per batch. Offsets received after the nack must never commit.
    let mut t = tracker();
    t.on_receive(P, 10);
    t.on_receive(P, 11);
    assert_eq!(t.on_ack(P, 10), Some(10));
    t.on_nack(P, 11);
    // Next "batch" arrives.
    t.on_receive(P, 12);
    t.on_receive(P, 13);
    assert_eq!(t.on_ack(P, 12), None);
    assert_eq!(t.on_ack(P, 13), None);
}

#[test]
fn memory_stays_bounded_after_a_nack() {
    // Offsets above the pin are neither tracked in-flight nor accumulated as
    // pending: sustained traffic after a nack must not grow state.
    let mut t = tracker();
    t.on_receive(P, 1);
    t.on_nack(P, 1);
    for o in 2..10_000 {
        t.on_receive(P, o);
        assert_eq!(t.on_ack(P, o), None);
    }
    // Indirect bound check: acks above the pin are ignored as untracked
    // (on_receive skipped them), so a duplicate ack also returns None.
    assert_eq!(t.on_ack(P, 9_999), None);
}

#[test]
fn duplicate_or_stale_acks_are_ignored() {
    let mut t = tracker();
    t.on_receive(P, 5);
    t.on_receive(P, 6);
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 5), None);
    assert_eq!(t.on_ack(P, 6), Some(6));
}

#[test]
fn ack_for_unseen_partition_returns_none() {
    let mut t = tracker();
    assert_eq!(t.on_ack(P, 5), None);
}

#[test]
fn partitions_are_tracked_independently() {
    let mut t = tracker();
    t.on_receive(0, 10);
    t.on_receive(1, 3);
    t.on_nack(0, 10);
    assert_eq!(t.on_ack(1, 3), Some(3));
}

#[test]
fn remove_partition_resets_tracking_for_rebalance() {
    // Revoke/reassign: redelivered offsets must re-track from scratch instead
    // of being suppressed by the stale `stored` guard.
    let mut t = tracker();
    for o in 400..=500 {
        t.on_receive(P, o);
        t.on_ack(P, o);
    }
    // Partition revoked, then reassigned; broker redelivers from 400.
    t.remove_partition(&P);
    t.on_receive(P, 400);
    assert_eq!(t.on_ack(P, 400), Some(400));
}

#[test]
fn works_with_unsigned_types() {
    // Iggy uses u32 partition ids and u64 offsets.
    let mut t: WatermarkTracker<u32, u64> = WatermarkTracker::new();
    t.on_receive(1u32, 5u64);
    t.on_receive(1u32, 6u64);
    assert_eq!(t.on_ack(1u32, 6u64), None);
    assert_eq!(t.on_ack(1u32, 5u64), Some(6));
}
