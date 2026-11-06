//! The cadence provider is called once at registration, synchronously — an
//! `async fn` is rejected up front rather than failing inside generated code.

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    async fn sync_cadence(&self) -> ScheduleConfig {
        "5m".parse().unwrap()
    }

    #[scheduled(schedule = "sync_cadence")]
    async fn tick(&self) {}
}

fn main() {}
