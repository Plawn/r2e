use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(state = AppState)]
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
}

fn main() {}
