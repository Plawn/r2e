//! `#[scheduled(skip_if = "...")]` — the declarative skip predicate
//! (Quarkus `skipExecutionIf`) on controllers and beans.
//!
//! The predicate is a plain `&self` method (sync or async) returning `bool`
//! on the same impl block, evaluated before every tick; `true` suppresses the
//! body and counts in `ScheduledJobInfo::skip_count`. To gate on a shared
//! condition, `#[inject]` the predicate bean and delegate to it.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::builder::ScheduledTaskMarker;
use r2e::prelude::*;
use r2e::r2e_executor::{Executor, ExecutorConfig, PoolExecutor};
use r2e::r2e_scheduler::{
    extract_tasks, start_jobs, ScheduledJobRegistry, Scheduler, SchedulerCommands,
};
use r2e::Controller as ControllerTrait;
use r2e::TaskRegistryHandle;
use tokio_util::sync::CancellationToken;

// ─── Helper: call the generated `scheduled_tasks_boxed` while letting the
// compiler infer the extraction-marker witness `W` (same pattern as
// scheduled_test.rs). ───

trait ScheduledExt<S, W>: Sized {
    fn boxed_tasks(state: &S, core: Arc<Self>, ctx: &r2e::BeanContext) -> Vec<Box<dyn Any + Send>>;
}

impl<C, S, W> ScheduledExt<S, W> for C
where
    C: ControllerTrait<S, W>,
    S: Clone + Send + Sync + 'static,
{
    fn boxed_tasks(state: &S, core: Arc<Self>, ctx: &r2e::BeanContext) -> Vec<Box<dyn Any + Send>> {
        <C as ControllerTrait<S, W>>::scheduled_tasks_boxed(state, core, ctx)
    }
}

// ─── Controller with a SYNC skip predicate ───

#[controller]
pub struct GatedScheduled {
    #[inject]
    ticks: Arc<AtomicUsize>,
    #[inject]
    maintenance: Arc<AtomicBool>,
}

#[routes]
impl GatedScheduled {
    // Plain `&self -> bool` method on the same impl block.
    fn in_maintenance(&self) -> bool {
        self.maintenance.load(Ordering::SeqCst)
    }

    #[scheduled(every = "50ms", skip_if = "in_maintenance", name = "gated_tick")]
    async fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Bean with an ASYNC skip predicate ───

#[derive(Clone)]
pub struct GatedBean {
    ticks: Arc<AtomicUsize>,
    maintenance: Arc<AtomicBool>,
}

#[bean]
impl GatedBean {
    pub fn new(ticks: Arc<AtomicUsize>, maintenance: Arc<AtomicBool>) -> Self {
        Self { ticks, maintenance }
    }

    async fn paused(&self) -> bool {
        self.maintenance.load(Ordering::SeqCst)
    }

    #[scheduled(every = "50ms", skip_if = "paused", name = "gated_bean_tick")]
    async fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[r2e::test]
async fn controller_skip_if_gates_ticks() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let maintenance = Arc::new(AtomicBool::new(true));
    let builder = AppBuilder::new()
        .provide(ticks.clone())
        .provide(maintenance.clone())
        .build_state()
        .await;
    let core = Arc::new(GatedScheduled::from_context(builder.bean_context()));

    let boxed = GatedScheduled::boxed_tasks(builder.state(), core, builder.bean_context());
    let tasks = extract_tasks(boxed);
    assert_eq!(tasks.len(), 1);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    // Maintenance on: every tick is skipped.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ticks.load(Ordering::SeqCst), 0, "ticks gated by skip_if");
    let info = registry.job("gated_tick").expect("job registered");
    assert!(info.skip_count >= 2, "got {}", info.skip_count);
    assert_eq!(info.run_count, 0);

    // Maintenance off: the task runs again.
    maintenance.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "ticks resume after the gate clears"
    );
}

#[r2e::test]
async fn bean_async_skip_if_gates_ticks() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let maintenance = Arc::new(AtomicBool::new(true));
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .provide(maintenance.clone())
        .register::<GatedBean>()
        .build_state()
        .await;

    let task_registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(task_registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "gated_bean_tick");

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        0,
        "async predicate gates the bean task"
    );
    let info = registry.job("gated_bean_tick").expect("job registered");
    assert!(info.skip_count >= 2);
    assert_eq!(info.run_count, 0);

    maintenance.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "bean ticks resume after the gate clears"
    );
}
