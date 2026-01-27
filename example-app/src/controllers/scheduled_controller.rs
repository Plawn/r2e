use crate::services::UserService;
use crate::state::Services;
use quarlus_core::prelude::*;

#[derive(Controller)]
#[controller(state = Services)]
pub struct ScheduledJobs {
    #[inject]
    user_service: UserService,
}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }
}
