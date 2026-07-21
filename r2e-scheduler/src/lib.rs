//! Background task scheduler for R2E.
//!
//! Provides interval, cron, and delayed task execution. Install with
//! `.plugin(Scheduler)` before `build_state()`.

mod driver;
mod duration;
mod types;

pub use driver::{start_jobs, SchedulerCommands};
pub use duration::{parse_duration, PositiveDuration};
pub use types::{
    extract_tasks, OverlapPolicy, ScheduleConfig, ScheduleParseError, ScheduledJob,
    ScheduledResult, ScheduledTask, ScheduledTaskDef, SkipFn,
};

use std::any::Any;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use driver::Command;
use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::StatusCode;
use r2e_core::prelude::ConfigProperties;
use r2e_core::{AppBuilder, BeanContext, DeferredContext, PluginInstallContext, PreStatePlugin};
use r2e_executor::{ExecutorConfig, PoolExecutor};

/// Handle to the scheduler runtime.
///
/// Can be extracted as an Axum handler parameter to check scheduler status
/// or trigger cancellation.
///
/// # Example
///
/// ```ignore
/// #[get("/scheduler/status")]
/// async fn status(&self, scheduler: SchedulerHandle) -> Json<bool> {
///     Json(scheduler.is_cancelled())
/// }
/// ```
#[derive(Clone)]
pub struct SchedulerHandle {
    cancel: CancellationToken,
    /// Command channel to the driver. `None` for handles built via
    /// [`new`](Self::new) without a driver (runtime control is then a no-op).
    commands: Option<mpsc::Sender<Command>>,
}

impl SchedulerHandle {
    /// Create a new scheduler handle from a cancellation token.
    ///
    /// This handle carries no command channel, so [`pause`](Self::pause),
    /// [`resume`](Self::resume), and [`trigger_now`](Self::trigger_now) all
    /// return `false`. The [`Scheduler`] plugin installs a fully-wired handle.
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            commands: None,
        }
    }

    /// Create a handle wired to the driver's command channel (plugin-internal).
    pub(crate) fn with_commands(
        cancel: CancellationToken,
        commands: mpsc::Sender<Command>,
    ) -> Self {
        Self {
            cancel,
            commands: Some(commands),
        }
    }

    /// Build a handle paired with the [`SchedulerCommands`] receiver to hand to
    /// [`start_jobs`].
    ///
    /// The [`Scheduler`] plugin wires this automatically; reach for it only when
    /// driving [`start_jobs`] manually and you want runtime control
    /// ([`pause`](Self::pause) / [`resume`](Self::resume) /
    /// [`trigger_now`](Self::trigger_now)) to work.
    pub fn channel(cancel: CancellationToken) -> (SchedulerHandle, SchedulerCommands) {
        let (tx, rx) = mpsc::channel(64);
        (
            SchedulerHandle::with_commands(cancel, tx),
            SchedulerCommands::new(rx),
        )
    }

    /// Cancel the scheduler and all running tasks.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Check if the scheduler has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Get the underlying cancellation token.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Pause a scheduled job by name. A paused job keeps advancing its cadence
    /// but never fires until [`resume`](Self::resume)d.
    ///
    /// Returns `false` if the job is unknown or this handle has no driver.
    pub async fn pause(&self, name: &str) -> bool {
        self.send(|reply| Command::Pause {
            name: name.to_string(),
            reply,
        })
        .await
    }

    /// Resume a paused job by name. Returns `false` if the job is unknown or
    /// this handle has no driver.
    pub async fn resume(&self, name: &str) -> bool {
        self.send(|reply| Command::Resume {
            name: name.to_string(),
            reply,
        })
        .await
    }

    /// Fire a job once, immediately and out of band — allowed even when the job
    /// is paused. The out-of-band tick does not disturb the regular schedule.
    ///
    /// Returns `false` if the job is unknown, this handle has no driver, or the
    /// job uses [`OverlapPolicy::Skip`] and a tick is already in flight.
    pub async fn trigger_now(&self, name: &str) -> bool {
        self.send(|reply| Command::TriggerNow {
            name: name.to_string(),
            reply,
        })
        .await
    }

    async fn send(&self, make: impl FnOnce(tokio::sync::oneshot::Sender<bool>) -> Command) -> bool {
        let Some(tx) = &self.commands else {
            return false;
        };
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if tx.send(make(reply_tx)).await.is_err() {
            return false;
        }
        reply_rx.await.unwrap_or(false)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for SchedulerHandle {
    type Rejection = (StatusCode, &'static str);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            parts.extensions.get::<SchedulerHandle>().cloned().ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Scheduler not installed. Add `.plugin(Scheduler)` before build_state().",
            ))
        }
    }
}

// ── ScheduledJobRegistry ──────────────────────────────────────────────────

/// Information about a registered scheduled job, including live runtime stats.
///
/// The metadata (`name`, `schedule`) is fixed at registration; the remaining
/// fields are updated by the driver as the job runs. Timestamps are wall-clock
/// ([`chrono::DateTime<Utc>`]) since they are user-facing.
#[derive(Clone, Debug)]
pub struct ScheduledJobInfo {
    /// The name of the scheduled task.
    pub name: String,
    /// Human-readable schedule description (e.g., "every 30s", "cron: 0 */5 * * * *").
    pub schedule: String,
    /// Wall-clock time the job most recently fired, or `None` if it never has.
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    /// Wall duration of the most recent completed tick (submit → completion).
    pub last_duration: Option<Duration>,
    /// Wall-clock time the job is next expected to fire, or `None` when the
    /// schedule is exhausted (a spent cron).
    pub next_run: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of ticks whose body actually ran (scheduled and trigger-now).
    /// Ticks suppressed by a skip predicate count in [`skip_count`](Self::skip_count) instead.
    pub run_count: u64,
    /// Number of ticks suppressed by the job's skip predicate
    /// (`#[scheduled(skip_if = "...")]` / [`ScheduledTaskDef::with_skip_if`]).
    pub skip_count: u64,
    /// Number of ticks that panicked (contained by the pool).
    pub panic_count: u64,
    /// Whether the job is currently paused.
    pub paused: bool,
}

impl ScheduledJobInfo {
    /// Create a job info entry with zeroed stats.
    pub fn new(name: impl Into<String>, schedule: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schedule: schedule.into(),
            last_run: None,
            last_duration: None,
            next_run: None,
            run_count: 0,
            skip_count: 0,
            panic_count: 0,
            paused: false,
        }
    }
}

/// Registry of scheduled jobs, queryable at runtime.
///
/// Provided as a bean by the [`Scheduler`] plugin — inject it via `#[inject]`
/// to list registered jobs at runtime (e.g., for admin endpoints).
///
/// # Example
///
/// ```ignore
/// #[controller(path = "/admin", state = Services)]
/// pub struct AdminController {
///     #[inject] jobs: ScheduledJobRegistry,
/// }
///
/// #[routes]
/// impl AdminController {
///     #[get("/jobs")]
///     async fn list_jobs(&self) -> Json<Vec<ScheduledJobInfo>> {
///         Json(self.jobs.list_jobs())
///     }
/// }
/// ```
#[derive(Clone)]
pub struct ScheduledJobRegistry {
    inner: Arc<Mutex<Vec<ScheduledJobInfo>>>,
}

impl ScheduledJobRegistry {
    /// Create a new empty job registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a job in the registry.
    pub fn register(&self, info: ScheduledJobInfo) {
        self.inner.lock().unwrap().push(info);
    }

    /// List all registered jobs (a snapshot of their current stats).
    pub fn list_jobs(&self) -> Vec<ScheduledJobInfo> {
        self.inner.lock().unwrap().clone()
    }

    /// Snapshot of a single job by name.
    pub fn job(&self, name: &str) -> Option<ScheduledJobInfo> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.name == name)
            .cloned()
    }

    /// Insert a bare entry for `name` if none exists yet (idempotent).
    pub(crate) fn upsert(&self, name: &str, schedule: &str) {
        let mut g = self.inner.lock().unwrap();
        if !g.iter().any(|i| i.name == name) {
            g.push(ScheduledJobInfo::new(name, schedule));
        }
    }

    /// Mutate the entry named `name` in place (no-op if absent). Used by the
    /// driver to keep runtime stats current.
    #[doc(hidden)] // pub for tests only; not part of the public API
    pub fn update_job(&self, name: &str, f: impl FnOnce(&mut ScheduledJobInfo)) {
        let mut g = self.inner.lock().unwrap();
        if let Some(info) = g.iter_mut().find(|i| i.name == name) {
            f(info);
        }
    }
}

impl Default for ScheduledJobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Scheduler plugin — provides `CancellationToken` and `ScheduledJobRegistry`,
/// and installs the scheduler runtime.
///
/// Install this plugin **before** `build_state()` to:
/// - Provide a `CancellationToken` bean for injection via `#[inject]`
/// - Provide a [`ScheduledJobRegistry`] bean for querying registered jobs
/// - Automatically set up the scheduler backend
///
/// This plugin provides **two** beans (`CancellationToken` and
/// `ScheduledJobRegistry`) via a tuple `Provided` type.
///
/// # Requires the Executor plugin
///
/// Scheduled tick bodies run on the shared [`PoolExecutor`] (the Quarkus
/// model — one pool for background work), not inline in each job's timer loop.
/// The Scheduler therefore **requires** a `PoolExecutor` bean in the final
/// graph — install `.plugin(Executor)` (or any provider of `PoolExecutor`)
/// somewhere in the builder chain. This is enforced at compile time via the
/// plugin's `LateDeps`: a missing `PoolExecutor` is a guided compile error at
/// `build_state()`, not a runtime failure. Running ticks on the pool also means
/// a panicking tick is contained in its pool job — the schedule loop logs and
/// keeps ticking instead of dying.
///
/// # Example
///
/// ```ignore
/// use r2e_scheduler::Scheduler;
/// use r2e_executor::Executor;
///
/// AppBuilder::new()
///     .plugin(Scheduler)  // Before build_state()!
///     .plugin(Executor)   // Required: scheduled ticks run on the shared pool
///     .build_state()
///     .await
///     .register_controller::<ScheduledJobs>()
///     .serve("0.0.0.0:3000")
/// ```
///
/// In controllers, inject the token or job registry directly:
///
/// ```ignore
/// #[controller(state = Services)]
/// pub struct MyController {
///     #[inject] cancel: CancellationToken,
///     #[inject] jobs: ScheduledJobRegistry,
/// }
/// ```
///
/// Or extract the `SchedulerHandle` as a handler parameter:
///
/// ```ignore
/// #[get("/status")]
/// async fn status(&self, scheduler: SchedulerHandle) -> Json<bool> {
///     Json(scheduler.is_cancelled())
/// }
/// ```
pub struct Scheduler;

/// Typed configuration for the [`Scheduler`] plugin, read from the `scheduler.*`
/// YAML section.
///
/// Every field is optional. `executor` selects which pool ticks run on:
/// - `"shared"` (default) — the app-wide [`PoolExecutor`] from the `Executor`
///   plugin (resolved via `LateDeps`).
/// - `"dedicated"` — a private pool sized by the keys below, so scheduled work
///   never contends with other background jobs. The sizing keys are used **only**
///   in dedicated mode (ignored under `shared`) and mirror `ExecutorConfig`.
///
/// An unrecognized `executor` value is a boot panic.
///
/// ```yaml
/// scheduler:
///   enabled: true            # standard <prefix>.enabled gate (see below)
///   executor: dedicated
///   max-concurrent: 8        # dedicated only
///   queue-capacity: 256      # dedicated only
///   shutdown-timeout: 10s    # dedicated only
/// ```
///
/// Because [`CONFIG_PREFIX`](Scheduler) is `Some("scheduler")`, the standard
/// `scheduler.enabled = false` gate applies: the plugin's post-state effects
/// (starting tasks) are skipped, while its provided beans remain in the graph.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct SchedulerConfig {
    /// Pool selection: `"shared"` (default) or `"dedicated"`.
    #[config(key = "executor")]
    pub executor: Option<String>,
    /// Dedicated-pool max concurrency (mirrors `ExecutorConfig::max_concurrent`).
    #[config(key = "max-concurrent")]
    pub max_concurrent: Option<u64>,
    /// Dedicated-pool queue capacity (mirrors `ExecutorConfig::queue_capacity`).
    #[config(key = "queue-capacity")]
    pub queue_capacity: Option<u64>,
    /// Dedicated-pool graceful-shutdown timeout (mirrors `ExecutorConfig::shutdown_timeout`).
    #[config(key = "shutdown-timeout")]
    pub shutdown_timeout: Option<Duration>,
}

impl PreStatePlugin for Scheduler {
    type Provided = (CancellationToken, ScheduledJobRegistry);
    type Deps = ();
    // `PoolExecutor` stays a hard `LateDeps` requirement even when the config
    // selects a dedicated pool — a type-level requirement cannot be made
    // config-conditional. In dedicated mode the shared pool is simply not used
    // to run ticks (a private pool is built instead).
    type LateDeps = (PoolExecutor,);
    type Config = SchedulerConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("scheduler");

    fn install(
        &mut self,
        (): (),
        ctx: &mut PluginInstallContext<'_>,
    ) -> (CancellationToken, ScheduledJobRegistry) {
        let token = CancellationToken::new();
        let job_registry = ScheduledJobRegistry::new();

        // Runtime command channel: sender lives on the handle (extension),
        // receiver is stashed for `configure` to thread into the driver.
        let (handle, commands) = SchedulerHandle::channel(token.clone());
        let task_registry = TaskRegistryHandle::new();
        let cancel_for_stopper = token.clone();

        // Add the layer that provides SchedulerHandle via extension.
        ctx.add_layer(move |router| router.layer(r2e_core::http::Extension(handle)));

        // Store the task registry for use during controller registration.
        ctx.store_data(task_registry);
        // Hand the command receiver to `configure` (picked up via take_data).
        ctx.store_data(commands);

        // Register shutdown hook.
        ctx.on_shutdown(move || {
            cancel_for_stopper.cancel();
        });

        (token, job_registry)
    }

    fn configure(
        self,
        (token, job_registry): &(CancellationToken, ScheduledJobRegistry),
        (shared_executor,): (PoolExecutor,),
        config: Option<SchedulerConfig>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let token = token.clone();
        let job_registry = job_registry.clone();
        // Pick up the command receiver stored at install; fall back to a
        // disconnected handle if it's somehow absent (keeps the driver inert).
        let commands = ctx
            .take_data::<SchedulerCommands>()
            .unwrap_or_else(SchedulerCommands::disconnected);

        // Resolve which pool ticks run on. Dedicated mode builds a private pool
        // and registers its own graceful drain.
        let executor = resolve_executor(config, shared_executor, ctx);

        // Register a serve hook to start scheduled tasks. Drains only
        // scheduler-owned tasks from the shared registry so hooks for
        // other subsystems don't see them.
        ctx.on_serve(move |serve_ctx| {
            let tasks = serve_ctx.task_registry().take_of::<ScheduledTaskMarker>();
            start_scheduled_tasks(tasks, token, job_registry, executor, commands);
        });
    }
}

/// Resolve the [`PoolExecutor`] scheduled ticks run on from the `scheduler.*`
/// config: the shared pool (default) or a private, dedicated pool.
///
/// An unrecognized `executor` value panics at boot, consistent with plugin
/// config validation style.
fn resolve_executor(
    config: Option<SchedulerConfig>,
    shared: PoolExecutor,
    ctx: &mut DeferredContext<'_>,
) -> PoolExecutor {
    let config = config.unwrap_or_default();
    match config.executor.as_deref() {
        None | Some("shared") => shared,
        Some("dedicated") => {
            let defaults = ExecutorConfig::default();
            let exec_config = ExecutorConfig {
                max_concurrent: config.max_concurrent.unwrap_or(defaults.max_concurrent),
                queue_capacity: config.queue_capacity.unwrap_or(defaults.queue_capacity),
                shutdown_timeout: config.shutdown_timeout.unwrap_or(defaults.shutdown_timeout),
            };
            let timeout = exec_config.shutdown_timeout;
            let pool = PoolExecutor::new(exec_config);
            let drain = pool.clone();
            // Drain the private pool on shutdown (mirrors the Executor plugin).
            ctx.on_shutdown_async(move || async move {
                if timeout.is_zero() {
                    drain.shutdown();
                } else {
                    let _ = drain.shutdown_graceful(timeout).await;
                }
            });
            pool
        }
        Some(other) => panic!(
            "Invalid `scheduler.executor` value {other:?}: expected \"shared\" or \"dedicated\""
        ),
    }
}

/// Extension trait for `AppBuilder` to register scheduled tasks dynamically —
/// the runtime counterpart of `#[scheduled]` for tasks whose set is only known
/// at startup (e.g. driven by configuration).
///
/// Requires `.plugin(Scheduler)` before `build_state()`; tasks registered here
/// are started by the scheduler's serve hook alongside `#[scheduled]` tasks
/// (and show up in [`ScheduledJobRegistry`]). Registration must happen before
/// `serve()` — the task registry is drained once at serve time.
///
/// # Example
///
/// ```ignore
/// use r2e_scheduler::{AppBuilderSchedulerExt, ScheduledTaskDef, Scheduler};
///
/// let app = AppBuilder::new()
///     .plugin(Scheduler)
///     .provide(sync_service.clone())
///     .build_state()
///     .await;
///
/// // e.g. one sync task per configured source
/// let app = sources.iter().fold(app, |app, source| {
///     let svc = sync_service.clone();
///     let source = source.clone();
///     app.schedule_task(ScheduledTaskDef::new(
///         format!("sync_{}", source.name),
///         source.schedule.clone(), // ScheduleConfig, e.g. from #[config(...)]
///         svc,
///         move |svc| {
///             let source = source.clone();
///             async move { svc.sync(&source).await }
///         },
///     ))
/// });
/// ```
pub trait AppBuilderSchedulerExt: Sized {
    /// Register a single dynamic scheduled task.
    ///
    /// # Panics
    ///
    /// Panics if the [`Scheduler`] plugin was not installed before
    /// `build_state()`.
    fn schedule_task<T: Clone + Send + Sync + 'static>(self, task: ScheduledTaskDef<T>) -> Self {
        self.schedule_tasks([task])
    }

    /// Register a batch of dynamic scheduled tasks.
    ///
    /// # Panics
    ///
    /// Panics if the [`Scheduler`] plugin was not installed before
    /// `build_state()`.
    fn schedule_tasks<T: Clone + Send + Sync + 'static>(
        self,
        tasks: impl IntoIterator<Item = ScheduledTaskDef<T>>,
    ) -> Self;

    /// Register a single dynamic scheduled task built from the resolved bean
    /// graph — the closure receives the [`BeanContext`] so task state can be
    /// pulled by type instead of threaded through a `let` at the call site.
    ///
    /// ```ignore
    /// app.schedule_task_with(|ctx| ScheduledTaskDef::new(
    ///     "sync_users",
    ///     "5m".parse().unwrap(),
    ///     ctx.get::<SyncService>(),
    ///     |svc| async move { svc.sync().await },
    /// ))
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the [`Scheduler`] plugin was not installed before
    /// `build_state()`, or if the closure requests a bean that is not in the
    /// graph (`BeanContext::get` panics; use `try_get` for optional beans).
    fn schedule_task_with<T, F>(self, build: F) -> Self
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce(&BeanContext) -> ScheduledTaskDef<T>,
    {
        self.schedule_tasks_with(move |ctx| [build(ctx)])
    }

    /// Register a batch of dynamic scheduled tasks built from the resolved
    /// bean graph — the config-driven case in one call:
    ///
    /// ```ignore
    /// app.schedule_tasks_with(|ctx| {
    ///     let svc = ctx.get::<SyncService>();
    ///     sources
    ///         .iter()
    ///         .map(|source| {
    ///             let source = source.clone();
    ///             ScheduledTaskDef::new(
    ///                 format!("sync_{}", source.name),
    ///                 source.schedule.clone(),
    ///                 svc.clone(),
    ///                 move |svc| {
    ///                     let source = source.clone();
    ///                     async move { svc.sync(&source).await }
    ///                 },
    ///             )
    ///         })
    ///         .collect::<Vec<_>>()
    /// })
    /// ```
    ///
    /// # Panics
    ///
    /// Same conditions as [`schedule_task_with`](Self::schedule_task_with).
    fn schedule_tasks_with<T, I, F>(self, build: F) -> Self
    where
        T: Clone + Send + Sync + 'static,
        I: IntoIterator<Item = ScheduledTaskDef<T>>,
        F: FnOnce(&BeanContext) -> I;
}

impl<S: Clone + Send + Sync + 'static> AppBuilderSchedulerExt for AppBuilder<S> {
    fn schedule_tasks<T: Clone + Send + Sync + 'static>(
        self,
        tasks: impl IntoIterator<Item = ScheduledTaskDef<T>>,
    ) -> Self {
        let registry = self
            .get_plugin_data::<TaskRegistryHandle>()
            .expect(
                "Scheduler not installed. Add `.plugin(Scheduler)` before build_state() to register dynamic scheduled tasks.",
            )
            .clone();

        let boxed: Vec<_> = tasks
            .into_iter()
            .map(ScheduledTaskDef::into_boxed_any)
            .collect();
        registry.add_boxed_for::<ScheduledTaskMarker>(boxed);

        self
    }

    fn schedule_tasks_with<T, I, F>(self, build: F) -> Self
    where
        T: Clone + Send + Sync + 'static,
        I: IntoIterator<Item = ScheduledTaskDef<T>>,
        F: FnOnce(&BeanContext) -> I,
    {
        let tasks: Vec<_> = build(self.bean_context()).into_iter().collect();
        self.schedule_tasks(tasks)
    }
}

pub mod prelude {
    //! Re-exports of the most commonly used scheduler types.
    pub use crate::{
        AppBuilderSchedulerExt, OverlapPolicy, ScheduleConfig, ScheduledJobInfo,
        ScheduledJobRegistry, ScheduledTaskDef, Scheduler, SchedulerConfig, SchedulerHandle,
    };
}

/// Format a schedule config as a human-readable string.
pub(crate) fn format_schedule(config: &ScheduleConfig) -> String {
    match config {
        ScheduleConfig::Interval(d) => format!("every {}s", d.as_secs()),
        ScheduleConfig::IntervalWithDelay {
            interval,
            initial_delay,
        } => format!(
            "every {}s (delay {}s)",
            interval.as_secs(),
            initial_delay.as_secs()
        ),
        ScheduleConfig::Cron(expr) => format!("cron: {}", expr),
    }
}

/// Start scheduled tasks from boxed task definitions.
///
/// This function is called by the builder's serve() method. It receives:
/// - `boxed_tasks`: Type-erased task definitions (Vec<Box<dyn Any + Send>>)
/// - `token`: The cancellation token
/// - `job_registry`: Registry to populate with job metadata
/// - `executor`: The shared pool each tick body is submitted to
///
/// Tasks already have their state captured (via `ScheduledTaskDef.state`), so
/// no state parameter is needed here. The function extracts tasks by
/// downcasting to `Box<dyn ScheduledTask>`, populates the job registry with
/// metadata (read before conversion), converts every task into a
/// [`ScheduledJob`], and hands them to a single driver task via
/// [`start_jobs`] — all schedules share one driver backed by a min-heap of
/// next-fire times, not one Tokio task per schedule.
fn start_scheduled_tasks(
    boxed_tasks: Vec<Box<dyn Any + Send>>,
    token: CancellationToken,
    job_registry: ScheduledJobRegistry,
    executor: PoolExecutor,
    commands: SchedulerCommands,
) {
    let tasks = extract_tasks(boxed_tasks);
    if tasks.is_empty() {
        return;
    }

    // Populate the job registry with task metadata before conversion. The
    // driver then keeps the runtime stats on these entries current.
    for task in &tasks {
        job_registry.register(ScheduledJobInfo::new(
            task.name().to_string(),
            format_schedule(task.schedule()),
        ));
    }

    // Bind the count to a plain statement: `tracing::info!(field = expr, …)`
    // evaluates `expr` inside a macro-internal region that coverage tooling
    // does not attribute to this line even when executed.
    let count = tasks.len();
    tracing::info!(count, "Starting scheduled tasks");
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(jobs, token, executor, job_registry, commands);
}
