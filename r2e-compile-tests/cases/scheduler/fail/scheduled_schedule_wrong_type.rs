//! The cadence provider must return `ScheduleConfig`: the generated call
//! ascribes the type, so a `String`-returning method fails at the call site.

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    fn sync_cadence(&self) -> String {
        "5m".to_string()
    }

    #[scheduled(schedule = "sync_cadence")]
    async fn tick(&self) {}
}

fn main() {}
