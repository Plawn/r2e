//! `#[scheduled(schedule = "method")]`: the cadence is a plain sync `&self ->
//! ScheduleConfig` method on the same impl block, read once at registration.
//! Works on both the controller path (`__core`) and the bean path (`self`),
//! with the cadence coming from a `#[config]` field, an `Option<ScheduleConfig>`
//! default, or a computation.

use r2e::prelude::*;

#[controller]
pub struct PullSweep {
    #[config("ingest.schedule")]
    cadence: ScheduleConfig,
}

#[routes]
impl PullSweep {
    fn ingest_cadence(&self) -> ScheduleConfig {
        self.cadence.clone()
    }

    #[scheduled(schedule = "ingest_cadence")]
    async fn poll_sources(&self) {}

    // Other keys still compose with `schedule`.
    #[scheduled(schedule = "ingest_cadence", name = "sweep-named", overlap = "concurrent", skip_if = "paused")]
    async fn poll_named(&self) {}

    fn paused(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Cleanup {
    cadence: Option<ScheduleConfig>,
}

#[bean]
impl Cleanup {
    pub fn new(#[config("jobs.cleanup")] cadence: Option<ScheduleConfig>) -> Self {
        Self { cadence }
    }

    fn cleanup_cadence(&self) -> ScheduleConfig {
        self.cadence
            .clone()
            .unwrap_or_else(|| "1h".parse().expect("valid default"))
    }

    #[scheduled(schedule = "cleanup_cadence")]
    fn purge(&self) {}
}

fn main() {}
