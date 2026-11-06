//! `schedule = "..."` must name a plain sync `&self -> ScheduleConfig` method
//! in the same impl block — an unknown name is a targeted compile error.

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    #[scheduled(schedule = "sync_cadence")]
    async fn tick(&self) {}
}

fn main() {}
