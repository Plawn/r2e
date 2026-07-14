use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: LocalEventBus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetRequest {
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetReply {
    pub message: String,
}

#[controller]
pub struct GreetResponder {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl GreetResponder {
    // `retry` is a fan-out subscriber option — invalid on a responder
    // (non-`()` return type).
    #[consumer(bus = "event_bus", retry = 3)]
    async fn greet(&self, req: std::sync::Arc<GreetRequest>) -> GreetReply {
        GreetReply {
            message: format!("Hello, {}!", req.name),
        }
    }
}

fn main() {}
