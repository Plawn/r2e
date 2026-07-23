use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    #[scheduled(every = "50ms", overlap = "sometimes")]
    async fn bad_overlap(&self) {}
}

fn main() {}
