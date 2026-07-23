//! `skip_if = "..."` must name a plain `&self -> bool` method in the same
//! impl block — an unknown name is a targeted compile error, not a cryptic
//! failure inside generated code.

use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    #[scheduled(every = "50ms", skip_if = "maintenance_mode")]
    async fn tick(&self) {}
}

fn main() {}
