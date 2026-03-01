use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: LocalEventBus,
}

#[derive(Debug, Clone)]
pub struct MyEvent;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct MyConsumer {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl MyConsumer {
    #[consumer]
    async fn on_event(&self, event: Arc<MyEvent>) {
        let _ = event;
    }
}

fn main() {}
