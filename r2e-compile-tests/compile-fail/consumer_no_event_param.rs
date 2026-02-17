use r2e::prelude::*;
use r2e::r2e_events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: EventBus,
}

#[derive(Controller)]
#[controller(state = AppState)]
pub struct MyConsumer {
    #[inject]
    event_bus: EventBus,
}

#[routes]
impl MyConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_event(&self) {
        // missing event parameter
    }
}

fn main() {}
