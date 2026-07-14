use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
use std::sync::Arc;

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
    // Infallible responder: non-`()` return type → registered via respond.
    #[consumer(bus = "event_bus")]
    async fn greet(&self, req: Arc<GreetRequest>) -> GreetReply {
        GreetReply {
            message: format!("Hello, {}!", req.name),
        }
    }

    // Fallible responder: `Result<Resp, E>` where E: Display.
    #[consumer(bus = "event_bus")]
    async fn greet_checked(&self, req: Arc<GreetRequest>) -> Result<GreetReply, String> {
        if req.name.is_empty() {
            return Err("empty name".to_string());
        }
        Ok(GreetReply {
            message: format!("Hi, {}!", req.name),
        })
    }

    // Plain fan-out subscriber still works alongside responders.
    #[consumer(bus = "event_bus")]
    async fn observe(&self, req: Arc<GreetRequest>) {
        let _ = &req.name;
    }
}

fn main() {}
