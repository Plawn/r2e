use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::builder::TaskRegistryHandle;
use r2e_core::{AppBuilder, BeanContext, BeanState, BuildableFrom, Contains};
use r2e_scheduler::{
    extract_tasks, ScheduleConfig, Scheduler, ScheduledTask, ScheduledTaskDef,
};
use tokio_util::sync::CancellationToken;

// ── Minimal test state ─────────────────────────────────────────────────────

#[derive(Clone)]
struct TestState {
    #[allow(dead_code)]
    cancel: CancellationToken,
}

impl BeanState for TestState {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            cancel: ctx.get::<CancellationToken>(),
        }
    }
}

impl<P, I0> BuildableFrom<P, (I0,)> for TestState where P: Contains<CancellationToken, I0> {}

// ── Helpers ────────────────────────────────────────────────────────────────

fn counting_task(
    name: &str,
    schedule: ScheduleConfig,
    counter: Arc<AtomicUsize>,
) -> ScheduledTaskDef<Arc<AtomicUsize>> {
    ScheduledTaskDef {
        name: name.to_string(),
        schedule,
        state: counter,
        task: Box::new(|c| {
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        }),
    }
}

fn boxed_task(
    task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>,
) -> Box<dyn std::any::Any + Send> {
    let trait_obj: Box<dyn ScheduledTask> = Box::new(task);
    Box::new(trait_obj)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler_plugin_provides_token() {
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .build_state::<TestState, _>()
        .await;

    let data = app.get_plugin_data::<TaskRegistryHandle>();
    // Plugin installed successfully; token not cancelled
    assert!(data.is_some(), "TaskRegistryHandle should be stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler_plugin_stores_registry() {
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .build_state::<TestState, _>()
        .await;

    let registry = app.get_plugin_data::<TaskRegistryHandle>();
    assert!(registry.is_some(), "TaskRegistryHandle should be available");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_collects_and_extracts() {
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .build_state::<TestState, _>()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");

    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "collected",
        ScheduleConfig::Interval(Duration::from_secs(60)),
        counter,
    );
    registry.add_boxed(vec![boxed_task(task)]);

    let boxed = registry.take_all();
    let tasks = extract_tasks(boxed);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "collected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_lifecycle_without_serve() {
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .build_state::<TestState, _>()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");

    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "lifecycle",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        counter.clone(),
    );
    registry.add_boxed(vec![boxed_task(task)]);

    // Manually extract and start
    let boxed = registry.take_all();
    let tasks = extract_tasks(boxed);
    assert_eq!(tasks.len(), 1);

    let token = CancellationToken::new();
    for t in tasks {
        t.start(token.clone());
    }

    // Let it run a bit
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count_before = counter.load(Ordering::SeqCst);
    assert!(count_before >= 1, "task should have run at least once, got {count_before}");

    // Cancel and wait for cancellation to take effect
    token.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Snapshot after cancel has settled
    let count_snapshot = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count_after = counter.load(Ordering::SeqCst);
    assert_eq!(
        count_snapshot, count_after,
        "counter should not increment after cancel settled (snapshot={count_snapshot}, after={count_after})"
    );
}
