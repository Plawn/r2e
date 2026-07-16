//! `#[scheduled]` on `#[bean]` — W10 phase 1.
//!
//! Beans declare scheduled tasks with the same `#[scheduled]` attribute as
//! controllers; `#[bean]` generates a `ScheduledSource` impl and an
//! `after_register` hook, so `.register::<T>()` alone is enough:
//! `build_state()` collects the tasks into the scheduler's `TaskRegistryHandle`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::builder::ScheduledTaskMarker;
use r2e::prelude::*;
use r2e::r2e_executor::{Executor, ExecutorConfig, PoolExecutor};
use r2e::r2e_scheduler::{
    extract_tasks, start_jobs, ScheduledJobRegistry, Scheduler, SchedulerCommands,
};
use r2e::TaskRegistryHandle;
use tokio_util::sync::CancellationToken;

// ─── Scheduled bean ───

#[derive(Clone)]
pub struct CleanupBean {
    ticks: Arc<AtomicUsize>,
}

#[bean]
impl CleanupBean {
    pub fn new(ticks: Arc<AtomicUsize>) -> Self {
        Self { ticks }
    }

    #[scheduled(every = 1)]
    async fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }

    // Sync method with a Result return: `ScheduledResult::log_if_err` path.
    // Long interval — it only fires the initial tick during the test.
    #[scheduled(every = 3600, name = "sync_result_tick")]
    fn sync_tick(&self) -> Result<(), String> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ─── Bean combining #[post_construct] and #[scheduled] (merged after_register) ───

#[derive(Clone)]
pub struct WarmupBean {
    warmed: Arc<AtomicUsize>,
}

#[bean]
impl WarmupBean {
    pub fn new(warmed: Arc<AtomicUsize>) -> Self {
        Self { warmed }
    }

    #[post_construct]
    fn warm(&self) {
        self.warmed.fetch_add(100, Ordering::SeqCst);
    }

    #[scheduled(every = "1h", name = "warmup_refresh")]
    async fn refresh(&self) {
        self.warmed.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[r2e::test]
async fn bean_scheduled_tasks_are_collected_at_build_state() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .register::<CleanupBean>()
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("Scheduler plugin stores the task registry");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());

    let mut names: Vec<_> = tasks.iter().map(|t| t.name().to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        ["CleanupBean_tick", "sync_result_tick"],
        "both #[scheduled] methods collected, default + explicit names"
    );
}

#[r2e::test]
async fn bean_scheduled_tasks_run_on_the_scheduler() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .register::<CleanupBean>()
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 2);

    let cancel = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );

    // 1s interval task → >= 2 ticks after 2.5s; the 1h sync task fires its
    // initial tick once.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    cancel.cancel();

    let count = ticks.load(Ordering::SeqCst);
    assert!(
        count >= 3,
        "expected >= 3 ticks (2× interval + 1 initial sync), got {count}"
    );
}

#[r2e::test]
async fn bean_with_post_construct_and_scheduled_wires_both() {
    let warmed = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(warmed.clone())
        .register::<WarmupBean>()
        .build_state()
        .await;

    // post_construct ran during build_state
    assert_eq!(warmed.load(Ordering::SeqCst), 100);

    // and the scheduled task was still collected (merged after_register)
    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "warmup_refresh");
}

#[r2e::test]
async fn default_override_registration_schedules_tasks_once() {
    // `with_default_bean` + `register_override` of the SAME type run
    // `after_register` twice; the scheduled-source hook must stay unique per
    // type or every task would fire twice per tick.
    let ticks = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .with_default_bean::<CleanupBean>()
        .register_override::<CleanupBean>()
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(
        tasks.len(),
        2,
        "one hook per bean type — tasks must not be duplicated by the \
         default/override pattern"
    );
}

#[r2e::test]
async fn bean_scheduled_without_scheduler_plugin_is_a_warning_not_a_panic() {
    let ticks = Arc::new(AtomicUsize::new(0));
    // No Executor/Scheduler plugins: tasks are dropped with a warning.
    let app = AppBuilder::new()
        .provide(ticks.clone())
        .register::<CleanupBean>()
        .build_state()
        .await;

    assert!(app.get_plugin_data::<TaskRegistryHandle>().is_none());
}
