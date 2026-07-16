use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupService {
    bus: LocalEventBus,
}

#[derive(Clone)]
pub struct Ping;

#[bean]
impl CleanupService {
    pub fn new(bus: LocalEventBus) -> Self {
        Self { bus }
    }

    #[scheduled(every = 10)]
    #[consumer(bus = "bus")]
    async fn handle(&self, event: std::sync::Arc<Ping>) {
        let _ = event;
    }
}

fn main() {}
