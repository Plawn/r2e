//! `#[scheduled]` on `#[bean]` — the macro generates a `ScheduledSource`
//! impl and an `after_register` hook; `.register::<T>()` alone wires the
//! tasks at `build_state()`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupService {
    ticks: Arc<AtomicUsize>,
}

#[bean]
impl CleanupService {
    pub fn new(ticks: Arc<AtomicUsize>) -> Self {
        Self { ticks }
    }

    #[scheduled(every = "5m", initial_delay = "10s")]
    async fn purge(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }

    #[scheduled(cron = "0 */5 * * * *", name = "cron_purge")]
    fn cron_purge(&self) -> Result<(), String> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[scheduled(every = "50ms", overlap = "concurrent")]
    async fn concurrent_tick(&self) {}
}

// The generated impl is usable as a `ScheduledSource` bound.
fn assert_scheduled_source<T: ScheduledSource>() {}

fn main() {
    assert_scheduled_source::<CleanupService>();
}
