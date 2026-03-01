use std::sync::Arc;

use crate::models::UserCreatedEvent;
use crate::state::Services;
use r2e::prelude::*;

#[derive(Controller)]
#[controller(state = Services)]
pub struct UserEventConsumer {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
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
