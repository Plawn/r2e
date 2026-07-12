use r2e_events_kafka::offset_tracker::OffsetTracker;

const P: i32 = 0;

#[test]
fn in_order_completions_advance_watermark_each_step() {
    let mut t = OffsetTracker::new();
    for o in 5..=7 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 6), Some(6));
    assert_eq!(t.on_ack(P, 7), Some(7));
}

#[test]
fn out_of_order_completions_stored_only_on_contiguous_prefix() {
    let mut t = OffsetTracker::new();
    for o in 5..=7 {
        t.on_receive(P, o);
    }
    // 7 and 6 complete before 5 — nothing committable yet.
    assert_eq!(t.on_ack(P, 7), None);
    assert_eq!(t.on_ack(P, 6), None);
    // 5 completes and closes the gap: watermark jumps to the highest contiguous.
    assert_eq!(t.on_ack(P, 5), Some(7));
}

#[test]
fn partial_prefix_advances_to_gap_only() {
    let mut t = OffsetTracker::new();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 8), None); // hole at 5,6,7
    assert_eq!(t.on_ack(P, 5), Some(5)); // advances to 5, 6 still pending
    assert_eq!(t.on_ack(P, 7), None); // hole at 6
    assert_eq!(t.on_ack(P, 6), Some(8)); // closes gap: 6,7,8 all contiguous
}

#[test]
fn nack_stalls_watermark_and_higher_acks_stay_unstored() {
    let mut t = OffsetTracker::new();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    // 5 nacks — the watermark can never advance past it.
    t.on_nack(P, 5);
    assert_eq!(t.on_ack(P, 6), None);
    assert_eq!(t.on_ack(P, 7), None);
    assert_eq!(t.on_ack(P, 8), None);
}

#[test]
fn nack_above_watermark_stalls_at_the_nacked_offset() {
    let mut t = OffsetTracker::new();
    for o in 5..=8 {
        t.on_receive(P, o);
    }
    assert_eq!(t.on_ack(P, 5), Some(5)); // watermark at 5, next=6
    t.on_nack(P, 6); // 6 nacks
    assert_eq!(t.on_ack(P, 7), None); // stalled at 6
    assert_eq!(t.on_ack(P, 8), None);
}

#[test]
fn partitions_are_tracked_independently() {
    let mut t = OffsetTracker::new();
    t.on_receive(0, 10);
    t.on_receive(1, 3);
    assert_eq!(t.on_ack(1, 3), Some(3));
    assert_eq!(t.on_ack(0, 10), Some(10));
}

#[test]
fn ack_below_watermark_is_ignored() {
    let mut t = OffsetTracker::new();
    t.on_receive(P, 5);
    t.on_receive(P, 6);
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 6), Some(6));
    // A stale duplicate below the watermark must not re-store or regress.
    assert_eq!(t.on_ack(P, 5), None);
}

#[test]
fn ack_for_unseen_partition_returns_none() {
    let mut t = OffsetTracker::new();
    assert_eq!(t.on_ack(P, 5), None);
}

#[test]
fn offset_gaps_do_not_stall_the_watermark() {
    // Compacted topics and transactional control markers leave holes in the
    // offset sequence — only offsets actually received may gate the commit.
    let mut t = OffsetTracker::new();
    t.on_receive(P, 5);
    t.on_receive(P, 7);
    t.on_receive(P, 9);
    assert_eq!(t.on_ack(P, 5), Some(5));
    assert_eq!(t.on_ack(P, 9), None); // 7 still in flight
    assert_eq!(t.on_ack(P, 7), Some(9)); // gap at 6 and 8 is irrelevant
}
