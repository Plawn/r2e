//! `schedule` owns the whole cadence: combining it with a literal `every`
//! (or `cron`) is rejected rather than silently ignoring one of them.

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    fn sync_cadence(&self) -> ScheduleConfig {
        "5m".parse().unwrap()
    }

    #[scheduled(every = "50ms", schedule = "sync_cadence")]
    async fn tick(&self) {}
}

fn main() {}
