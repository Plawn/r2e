//! `#[scheduled]` and `#[consumer]` on ONE controller method is contradictory
//! (parity with the bean-side check).

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;

#[derive(Clone)]
pub struct AppState;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evt {
    pub n: u32,
}

#[controller]
pub struct Jobs {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl Jobs {
    #[scheduled(every = 5)]
    #[consumer(bus = "event_bus")]
    async fn both(&self, _e: std::sync::Arc<Evt>) {}
}

fn main() {}
