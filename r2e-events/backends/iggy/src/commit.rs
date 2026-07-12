use std::collections::{BTreeMap, HashSet};

use r2e_events::backend::DispatchOutcome;

/// Compute the highest offset safe to commit per partition from a batch's
/// dispatch outcomes (at-least-once delivery).
///
/// Outcomes must be supplied in poll order — offsets are monotonic per
/// partition. For each partition the returned offset is that of the last
/// message in the longest contiguous `Ack` prefix: the first `Nack` stops
/// advancement for its partition, so that message and everything after it in
/// the partition is left uncommitted and redelivered on restart. A partition
/// whose first outcome is `Nack` is absent from the result (nothing safe to
/// commit).
#[doc(hidden)]
pub fn compute_commit_offsets<I>(outcomes: I) -> BTreeMap<u32, u64>
where
    I: IntoIterator<Item = (u32, u64, DispatchOutcome)>,
{
    let mut commit: BTreeMap<u32, u64> = BTreeMap::new();
    let mut blocked: HashSet<u32> = HashSet::new();

    for (partition_id, offset, outcome) in outcomes {
        // Once a partition has a nacked message, nothing after it (higher
        // offset) may be committed — it must be redelivered.
        if blocked.contains(&partition_id) {
            continue;
        }
        match outcome {
            DispatchOutcome::Ack => {
                commit.insert(partition_id, offset);
            }
            DispatchOutcome::Nack => {
                blocked.insert(partition_id);
            }
        }
    }

    commit
}
