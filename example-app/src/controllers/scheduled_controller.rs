use crate::services::UserService;
use crate::state::Services;

quarlus_macros::controller! {
    impl ScheduledJobs for Services {
        #[inject]
        user_service: UserService,

        #[scheduled(every = 30)]
        async fn count_users(&self) {
            let count = self.user_service.count().await;
            tracing::info!(count, "Scheduled user count");
        }
    }
}
