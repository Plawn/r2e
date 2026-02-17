use r2e::prelude::*;
use r2e::r2e_events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: EventBus,
}

#[derive(Debug, Clone)]
pub struct MyEvent;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct MyConsumer {
    #[inject]
    event_bus: EventBus,
}

#[routes]
impl MyConsumer {
    #[consumer]
    async fn on_event(&self, event: Arc<MyEvent>) {
        let _ = event;
    }
}

fn main() {}
