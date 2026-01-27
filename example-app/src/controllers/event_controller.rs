use std::sync::Arc;

use crate::models::UserCreatedEvent;
use crate::state::Services;
use quarlus_events::EventBus;

#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct UserEventConsumer {
    #[inject]
    event_bus: EventBus,
}

#[quarlus_macros::routes]
impl UserEventConsumer {
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
