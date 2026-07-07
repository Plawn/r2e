use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_scheduler::extract_tasks;
use r2e::Controller as ControllerTrait;
use tokio_util::sync::CancellationToken;

// ─── Helper: call the generated `scheduled_tasks_boxed` while letting the
// compiler infer the extraction-marker witness `W`.
//
// In the state-generic model the `Controller<S, W>` impl carries opaque
// extraction markers in `W`, so a fully-qualified `<C as Controller<S>>::…`
// call no longer resolves. Parking `W` on a helper trait (the same pattern as
// `RegisterController`) lets registration/inference supply it. ───

trait ScheduledExt<S, W>: Sized {
    fn boxed_tasks(state: &S, core: Arc<Self>) -> Vec<Box<dyn Any + Send>>;
}

impl<C, S, W> ScheduledExt<S, W> for C
where
    C: ControllerTrait<S, W>,
    S: Clone + Send + Sync + 'static,
{
    fn boxed_tasks(state: &S, core: Arc<Self>) -> Vec<Box<dyn Any + Send>> {
        <C as ControllerTrait<S, W>>::scheduled_tasks_boxed(state, core)
    }
}

// ─── Scheduled controller ───

#[controller]
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

#[controller]
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
    let counter = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .provide(counter.clone())
        .build_state()
        .await;
    let core = Arc::new(IntervalCounter::from_context(builder.bean_context()));

    let cancel = CancellationToken::new();

    // Get scheduled task definitions from the controller (type-erased)
    let boxed_tasks = IntervalCounter::boxed_tasks(builder.state(), core);

    // Extract back to ScheduledTask trait objects
    let tasks = extract_tasks(boxed_tasks);
    assert!(!tasks.is_empty(), "Should have at least one scheduled task");

    // Start all tasks
    for task in tasks {
        task.start(cancel.clone());
    }

    // Wait for at least 2 ticks (interval = 1s, wait 2.5s)
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let count = counter.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Expected counter >= 2 after 2.5s with 1s interval, got {}",
        count
    );

    // Cancel and verify it stops
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count_after_cancel = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let count_later = counter.load(Ordering::SeqCst);

    assert_eq!(
        count_after_cancel, count_later,
        "Counter should not increase after cancellation"
    );
}

#[r2e::test]
async fn test_scheduled_cancellation_stops() {
    let counter = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .provide(counter.clone())
        .build_state()
        .await;
    let core = Arc::new(IntervalCounter::from_context(builder.bean_context()));

    let cancel = CancellationToken::new();

    let boxed_tasks = IntervalCounter::boxed_tasks(builder.state(), core);
    let tasks = extract_tasks(boxed_tasks);

    for task in tasks {
        task.start(cancel.clone());
    }

    // Let it run once
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let count_before = counter.load(Ordering::SeqCst);
    assert!(count_before >= 1, "Should have run at least once");

    // Cancel immediately
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let count_at_cancel = counter.load(Ordering::SeqCst);

    // Wait another interval period to ensure it stopped
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let count_after = counter.load(Ordering::SeqCst);

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
    let dep = CloneTrackedScheduledDep {
        clones: Arc::clone(&clones),
        ticks: Arc::clone(&ticks),
    };

    // The core injects the clone-tracked dep from the bean context (constructed
    // once). The scheduler clones the *state* on every tick, so we deliberately
    // pass a dep-free state to `boxed_tasks` — the state is unused by the task
    // body, and this keeps the clone counter tracking only core (re)construction.
    let ctx_builder = AppBuilder::new().provide(dep).build_state().await;
    let core = Arc::new(ReuseScheduledController::from_context(
        ctx_builder.bean_context(),
    ));

    let empty_builder = AppBuilder::new().build_state().await;
    let empty_state = empty_builder.state().clone();

    let boxed = ReuseScheduledController::boxed_tasks(&empty_state, core);

    // Baseline: every dep clone incurred while building state + constructing the
    // core has happened by now. A reused core must not add more.
    let base = clones.load(Ordering::SeqCst);

    let tasks = extract_tasks(boxed);
    let cancel = CancellationToken::new();
    for task in tasks {
        task.start(cancel.clone());
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    cancel.cancel();

    assert!(ticks.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        clones.load(Ordering::SeqCst),
        base,
        "scheduled ticks must not reconstruct the controller core"
    );
}
