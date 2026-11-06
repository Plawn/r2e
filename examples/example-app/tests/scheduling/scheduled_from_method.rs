//! `#[scheduled(schedule = "method")]` — the cadence comes from the bean
//! (Tasker #1181).
//!
//! The named method is a plain **sync** `&self -> ScheduleConfig` method on the
//! same impl block, called **once at registration** where the task definition
//! is built. The cadence can therefore come from a `#[config("…")]` field (with
//! the usual boot-time validation and dev-reload fingerprinting), an
//! `Option<ScheduleConfig>` with an in-code default, an injected policy bean,
//! or any computation. The `ScheduledJobRegistry` reflects the *resolved*
//! schedule, exactly as for a literal `every`/`cron`.

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::builder::ScheduledTaskMarker;
use r2e::config::{ConfigValue, R2eConfig};
use r2e::prelude::*;
use r2e::r2e_executor::{Executor, ExecutorConfig, PoolExecutor};
use r2e::r2e_scheduler::{
    extract_tasks, start_jobs, ScheduledJobRegistry, Scheduler, SchedulerCommands,
};
use r2e::Controller as ControllerTrait;
use r2e::TaskRegistryHandle;
use r2e::rt::CancelToken;

// ─── Helper: call the generated `scheduled_tasks_boxed` while letting the
// compiler infer the extraction-marker witness `W` (same pattern as
// scheduled_skip_if.rs). ───

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

fn config_with(key: &str, value: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set(key, ConfigValue::String(value.to_string()));
    config
}

// ─── Controller: cadence from a required `#[config]` field ───

#[controller]
pub struct ConfiguredSweep {
    #[inject]
    ticks: Arc<AtomicUsize>,
    #[config("jobs.cadence")]
    cadence: ScheduleConfig,
}

#[routes]
impl ConfiguredSweep {
    // Plain sync `&self -> ScheduleConfig` method on the same impl block.
    fn sweep_cadence(&self) -> ScheduleConfig {
        self.cadence.clone()
    }

    #[scheduled(schedule = "sweep_cadence", name = "configured_sweep")]
    async fn sweep(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Bean: `Option<ScheduleConfig>` with an in-code default ───

#[derive(Clone)]
pub struct ConfiguredCleanup {
    ticks: Arc<AtomicUsize>,
    cadence: Option<ScheduleConfig>,
}

#[bean]
impl ConfiguredCleanup {
    pub fn new(
        ticks: Arc<AtomicUsize>,
        #[config("jobs.cleanup")] cadence: Option<ScheduleConfig>,
    ) -> Self {
        Self { ticks, cadence }
    }

    fn cleanup_cadence(&self) -> ScheduleConfig {
        self.cadence
            .clone()
            .unwrap_or_else(|| "50ms".parse().expect("valid default cadence"))
    }

    #[scheduled(schedule = "cleanup_cadence", name = "configured_cleanup")]
    async fn purge(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[r2e::test]
async fn controller_cadence_comes_from_config() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .override_config(config_with("jobs.cadence", "50ms"))
        .load_config::<()>()
        .provide(ticks.clone())
        .build_state()
        .await;
    let core = Arc::new(ConfiguredSweep::from_context(builder.bean_context()));

    let boxed = ConfiguredSweep::boxed_tasks(builder.state(), core, builder.bean_context());
    let tasks = extract_tasks(boxed);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "configured_sweep");

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    r2e::rt::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "a 50ms cadence read from config ticks repeatedly, got {}",
        ticks.load(Ordering::SeqCst)
    );
    let info = registry.job("configured_sweep").expect("job registered");
    assert!(
        info.schedule.starts_with("every "),
        "registry shows the resolved interval, got {:?}",
        info.schedule
    );
}

/// Key absent → `Option<ScheduleConfig>` is `None` and the method's default
/// applies: the usual `#[config]` optional-key semantics, no new mechanism.
#[r2e::test]
async fn bean_cadence_falls_back_to_in_code_default() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .register::<ConfiguredCleanup>()
        .build_state()
        .await;

    let task_registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(task_registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "configured_cleanup");

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    r2e::rt::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "the in-code 50ms default ticks repeatedly, got {}",
        ticks.load(Ordering::SeqCst)
    );
}

/// Key present → it wins over the in-code default, and the registry reports
/// the resolved cron (not a literal from the attribute).
#[r2e::test]
async fn bean_cadence_reads_cron_from_config() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let app = AppBuilder::new()
        .override_config(config_with("jobs.cleanup", "0 0 3 * * *"))
        .load_config::<()>()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .register::<ConfiguredCleanup>()
        .build_state()
        .await;

    let task_registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("task registry present");
    let tasks = extract_tasks(task_registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    r2e::rt::sleep(Duration::from_millis(150)).await;
    cancel.cancel();

    let info = registry.job("configured_cleanup").expect("job registered");
    assert_eq!(info.schedule, "cron: 0 0 3 * * *");
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        0,
        "a nightly cron does not fire during the test"
    );
}

/// An invalid value never falls back silently: `ScheduleConfig: FromConfigValue`
/// rejects it while the core is constructed, naming the key — the same path
/// every `#[config]` field already takes.
#[r2e::test]
#[should_panic(expected = "key 'jobs.cadence'")]
async fn invalid_cadence_value_fails_construction() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .override_config(config_with("jobs.cadence", "soon"))
        .load_config::<()>()
        .provide(ticks)
        .build_state()
        .await;
    let _core = ConfiguredSweep::from_context(builder.bean_context());
}
