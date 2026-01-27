use crate::services::UserService;
use crate::state::Services;

#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct ScheduledJobs {
    #[inject]
    user_service: UserService,
}

#[quarlus_macros::routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }
}
