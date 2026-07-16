use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]
    async fn periodic_task(&self) {
        // runs every 30 seconds
    }

    #[scheduled(cron = "0 */5 * * * *")]
    async fn cron_task(&self) {
        // runs every 5 minutes
    }

    #[scheduled(every = 60, initial_delay = 10)]
    async fn delayed_task(&self) {
        // runs every 60s, first run after 10s delay
    }

    #[scheduled(every = "50ms", overlap = "concurrent")]
    async fn overlapping_task(&self) {
        // may overlap with itself under sustained load
    }

    #[scheduled(cron = "0 */5 * * * *", overlap = "skip")]
    async fn non_overlapping_cron(&self) {
        // explicit skip (the default) alongside cron
    }
}

fn main() {}
