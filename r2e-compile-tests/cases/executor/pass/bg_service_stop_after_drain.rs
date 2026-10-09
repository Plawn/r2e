//! `#[service(stop = "after_drain", order = N)]` on both struct shapes
//! (named fields and unit), with and without `enabled`, emits a
//! `ServiceComponent` whose `stop_phase` / `stop_order` read back what was
//! declared.

use r2e::prelude::*;
use r2e::rt::CancelToken;
use r2e::{ServiceComponent, StopPhase};

#[derive(Clone)]
pub struct Writer;

#[derive(BackgroundService, Clone)]
#[service(stop = "after_drain", order = 3, enabled = "enabled")]
pub struct Sink {
    #[inject]
    writer: Writer,
    #[config("sink.enabled")]
    enabled: bool,
}

impl Sink {
    async fn run(&self, shutdown: CancelToken) {
        let _ = &self.writer;
        shutdown.cancelled().await;
    }
}

#[derive(BackgroundService)]
#[service(stop = "after_drain")]
pub struct Flusher;

impl Flusher {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

#[derive(BackgroundService)]
#[service(stop = "early")]
pub struct Poller;

impl Poller {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

fn main() {
    assert_eq!(<Sink as ServiceComponent>::stop_phase(), StopPhase::AfterDrain);
    assert_eq!(<Sink as ServiceComponent>::stop_order(), 3);
    assert_eq!(<Flusher as ServiceComponent>::stop_phase(), StopPhase::AfterDrain);
    assert_eq!(<Flusher as ServiceComponent>::stop_order(), 0);
    assert_eq!(<Poller as ServiceComponent>::stop_phase(), StopPhase::Early);
}
