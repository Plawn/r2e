//! `initial_delay` cannot be layered on `schedule`: the method returns the full
//! `ScheduleConfig` (use `IntervalWithDelay` there instead).

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    fn sync_cadence(&self) -> ScheduleConfig {
        "5m".parse().unwrap()
    }

    #[scheduled(schedule = "sync_cadence", initial_delay = "10s")]
    async fn tick(&self) {}
}

fn main() {}
