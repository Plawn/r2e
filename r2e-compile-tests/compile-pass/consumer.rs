use r2e::prelude::*;
use r2e::r2e_events::EventBus;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: EventBus,
}

#[derive(Debug, Clone)]
pub struct UserCreated {
    pub name: String,
}

#[derive(Controller)]
#[controller(state = AppState)]
pub struct EventConsumer {
    #[inject]
    event_bus: EventBus,
}

#[routes]
impl EventConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreated>) {
        let _ = &event.name;
    }
}

fn main() {}
