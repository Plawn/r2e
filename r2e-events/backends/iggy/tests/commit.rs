use r2e_events::backend::DispatchOutcome::{Ack, Nack};
use r2e_events_iggy::compute_commit_offsets;

#[test]
fn all_acked_single_partition_commits_last_offset() {
    let commit = compute_commit_offsets([(0, 10, Ack), (0, 11, Ack), (0, 12, Ack)]);
    assert_eq!(commit.get(&0), Some(&12));
}

#[test]
fn nack_stops_at_contiguous_acked_prefix() {
    // offsets 10, 11 acked; 12 nacked; 13 acked but after the nack -> not committed.
    let commit = compute_commit_offsets([(0, 10, Ack), (0, 11, Ack), (0, 12, Nack), (0, 13, Ack)]);
    assert_eq!(commit.get(&0), Some(&11));
}

#[test]
fn leading_nack_leaves_partition_uncommitted() {
    let commit = compute_commit_offsets([(0, 10, Nack), (0, 11, Ack)]);
    assert_eq!(commit.get(&0), None);
}

#[test]
fn partitions_are_tracked_independently() {
    let commit = compute_commit_offsets([
        (0, 10, Ack),
        (1, 20, Ack),
        (0, 11, Nack),
        (1, 21, Ack),
        (0, 12, Ack), // after partition 0 is blocked -> ignored
    ]);
    assert_eq!(commit.get(&0), Some(&10));
    assert_eq!(commit.get(&1), Some(&21));
}

#[test]
fn empty_batch_commits_nothing() {
    let commit = compute_commit_offsets(std::iter::empty());
    assert!(commit.is_empty());
}
