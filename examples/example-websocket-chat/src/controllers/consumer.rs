use std::sync::Arc;

use r2e::prelude::*;

use crate::models::MessageSentEvent;
use crate::services::ChatService;
use crate::state::AppState;

/// Event consumer that persists chat messages to the database.
#[derive(Controller)]
#[controller(state = AppState)]
pub struct MessagePersistenceConsumer {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    chat_service: ChatService,
}

#[routes]
impl MessagePersistenceConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_message_sent(&self, event: Arc<MessageSentEvent>) {
        if let Err(e) = self
            .chat_service
            .save_message(&event.room, &event.username, &event.text)
            .await
        {
            tracing::error!("Failed to persist message: {e:?}");
        }
    }
}
