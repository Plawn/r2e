use std::sync::Arc;

use crate::models::UserCreatedEvent;
use crate::state::Services;
use quarlus_events::EventBus;

quarlus_macros::controller! {
    impl UserEventConsumer for Services {
        #[inject]
        event_bus: EventBus,

        #[consumer(bus = "event_bus")]
        async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
            tracing::info!(
                user_id = event.user_id,
                name = %event.name,
                email = %event.email,
                "User created event received"
            );
        }
    }
}
