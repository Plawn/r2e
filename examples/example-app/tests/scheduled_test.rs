use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_scheduler::extract_tasks;
use r2e::Controller as ControllerTrait;
use tokio_util::sync::CancellationToken;

// ─── State ───

#[derive(Clone)]
struct ScheduledTestState {
    counter: Arc<AtomicUsize>,
}

impl r2e::http::extract::FromRef<ScheduledTestState> for Arc<AtomicUsize> {
    fn from_ref(state: &ScheduledTestState) -> Self {
        state.counter.clone()
    }
}

// ─── Scheduled controller ───

#[derive(Controller)]
#[controller(state = ScheduledTestState)]
pub struct IntervalCounter {
    #[inject]
    counter: Arc<AtomicUsize>,
}

#[routes]
impl IntervalCounter {
    #[scheduled(every = 1)]
    async fn tick(&self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[tokio::test]
async fn test_scheduled_interval_runs() {
    let state = ScheduledTestState {
        counter: Arc::new(AtomicUsize::new(0)),
    };

    let cancel = CancellationToken::new();

    // Get scheduled task definitions from the controller (type-erased)
    let boxed_tasks =
        <IntervalCounter as ControllerTrait<ScheduledTestState>>::scheduled_tasks_boxed(&state);

    // Extract back to ScheduledTask trait objects
    let tasks = extract_tasks(boxed_tasks);
    assert!(!tasks.is_empty(), "Should have at least one scheduled task");

    // Start all tasks
    for task in tasks {
        task.start(cancel.clone());
    }

    // Wait for at least 2 ticks (interval = 1s, wait 2.5s)
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let count = state.counter.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Expected counter >= 2 after 2.5s with 1s interval, got {}",
        count
    );

    // Cancel and verify it stops
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count_after_cancel = state.counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let count_later = state.counter.load(Ordering::SeqCst);

    assert_eq!(
        count_after_cancel, count_later,
        "Counter should not increase after cancellation"
    );
}

#[tokio::test]
async fn test_scheduled_cancellation_stops() {
    let state = ScheduledTestState {
        counter: Arc::new(AtomicUsize::new(0)),
    };

    let cancel = CancellationToken::new();

    let boxed_tasks =
        <IntervalCounter as ControllerTrait<ScheduledTestState>>::scheduled_tasks_boxed(&state);
    let tasks = extract_tasks(boxed_tasks);

    for task in tasks {
        task.start(cancel.clone());
    }

    // Let it run once
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let count_before = state.counter.load(Ordering::SeqCst);
    assert!(count_before >= 1, "Should have run at least once");

    // Cancel immediately
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let count_at_cancel = state.counter.load(Ordering::SeqCst);

    // Wait another interval period to ensure it stopped
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let count_after = state.counter.load(Ordering::SeqCst);

    assert_eq!(
        count_at_cancel, count_after,
        "Task should have stopped after cancellation (was {}, now {})",
        count_at_cancel, count_after
    );
}
