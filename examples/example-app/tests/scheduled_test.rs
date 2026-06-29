use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_scheduler::extract_tasks;
use r2e::Controller as ControllerTrait;
use tokio_util::sync::CancellationToken;

// ─── State ───

#[derive(Clone, TestState)]
struct ScheduledTestState {
    counter: Arc<AtomicUsize>,
}

// ─── Scheduled controller ───

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

struct CloneTrackedScheduledDep {
    clones: Arc<AtomicUsize>,
    ticks: Arc<AtomicUsize>,
}

impl Clone for CloneTrackedScheduledDep {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
            ticks: Arc::clone(&self.ticks),
        }
    }
}

struct ReuseScheduledState {
    dep: CloneTrackedScheduledDep,
}

impl Clone for ReuseScheduledState {
    fn clone(&self) -> Self {
        Self {
            dep: CloneTrackedScheduledDep {
                clones: Arc::clone(&self.dep.clones),
                ticks: Arc::clone(&self.dep.ticks),
            },
        }
    }
}

#[controller(state = ReuseScheduledState)]
struct ReuseScheduledController {
    #[inject]
    dep: CloneTrackedScheduledDep,
}

#[routes]
impl ReuseScheduledController {
    #[scheduled(every = 1)]
    async fn tick(&self) {
        self.dep.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[r2e::test]
async fn test_scheduled_interval_runs() {
    let state = ScheduledTestState {
        counter: Arc::new(AtomicUsize::new(0)),
    };

    let cancel = CancellationToken::new();
    let core = Arc::new(IntervalCounter::from_state(&state));

    // Get scheduled task definitions from the controller (type-erased)
    let boxed_tasks =
        <IntervalCounter as ControllerTrait<ScheduledTestState>>::scheduled_tasks_boxed(
            &state, core,
        );

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

#[r2e::test]
async fn test_scheduled_cancellation_stops() {
    let state = ScheduledTestState {
        counter: Arc::new(AtomicUsize::new(0)),
    };

    let cancel = CancellationToken::new();
    let core = Arc::new(IntervalCounter::from_state(&state));

    let boxed_tasks =
        <IntervalCounter as ControllerTrait<ScheduledTestState>>::scheduled_tasks_boxed(
            &state, core,
        );
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

#[r2e::test]
async fn scheduled_task_reuses_supplied_core_for_every_tick() {
    let clones = Arc::new(AtomicUsize::new(0));
    let ticks = Arc::new(AtomicUsize::new(0));
    let state = ReuseScheduledState {
        dep: CloneTrackedScheduledDep {
            clones: Arc::clone(&clones),
            ticks: Arc::clone(&ticks),
        },
    };
    let core = Arc::new(ReuseScheduledController::from_state(&state));
    assert_eq!(clones.load(Ordering::SeqCst), 1);

    let tasks = extract_tasks(<ReuseScheduledController as ControllerTrait<
        ReuseScheduledState,
    >>::scheduled_tasks_boxed(&state, core));
    let cancel = CancellationToken::new();
    for task in tasks {
        task.start(cancel.clone());
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    cancel.cancel();

    assert!(ticks.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        clones.load(Ordering::SeqCst),
        1,
        "scheduled ticks must not reconstruct the controller core"
    );
}
