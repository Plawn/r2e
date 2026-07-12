//! Background task scheduler for R2E.
//!
//! Provides interval, cron, and delayed task execution. Install with
//! `.plugin(Scheduler)` before `build_state()`.

mod duration;
mod types;

pub use duration::parse_duration;
pub use types::{
    extract_tasks, ScheduleConfig, ScheduleParseError, ScheduledResult, ScheduledTask,
    ScheduledTaskDef,
};

use std::any::Any;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, BeanContext, PluginInstallContext, PreStatePlugin};

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
}

impl SchedulerHandle {
    /// Create a new scheduler handle from a cancellation token.
    pub fn new(cancel: CancellationToken) -> Self {
        Self { cancel }
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
}

impl<S: Send + Sync> FromRequestParts<S> for SchedulerHandle {
    type Rejection = (StatusCode, &'static str);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            parts
                .extensions
                .get::<SchedulerHandle>()
                .cloned()
                .ok_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Scheduler not installed. Add `.plugin(Scheduler)` before build_state().",
                ))
        }
    }
}

// ── ScheduledJobRegistry ──────────────────────────────────────────────────

/// Information about a registered scheduled job.
#[derive(Clone, Debug)]
pub struct ScheduledJobInfo {
    /// The name of the scheduled task.
    pub name: String,
    /// Human-readable schedule description (e.g., "every 30s", "cron: 0 */5 * * * *").
    pub schedule: String,
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

    /// List all registered jobs.
    pub fn list_jobs(&self) -> Vec<ScheduledJobInfo> {
        self.inner.lock().unwrap().clone()
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
/// # Example
///
/// ```ignore
/// use r2e_scheduler::Scheduler;
///
/// AppBuilder::new()
///     .plugin(Scheduler)  // Before build_state()!
///     .build_state::<Services, _>()
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

impl PreStatePlugin for Scheduler {
    type Provided = (CancellationToken, ScheduledJobRegistry);
    type Deps = ();
    type LateDeps = ();

    fn install(
        self,
        (): (),
        ctx: &mut PluginInstallContext<'_>,
    ) -> (CancellationToken, ScheduledJobRegistry) {
        let token = CancellationToken::new();
        let job_registry = ScheduledJobRegistry::new();
        let handle = SchedulerHandle::new(token.clone());
        let task_registry = TaskRegistryHandle::new();
        let cancel_for_stopper = token.clone();
        let token_for_serve = token.clone();
        let job_registry_for_serve = job_registry.clone();

        // Add the layer that provides SchedulerHandle via extension.
        ctx.add_layer(move |router| router.layer(r2e_core::http::Extension(handle)));

        // Store the task registry for use during controller registration.
        ctx.store_data(task_registry);

        // Register a serve hook to start scheduled tasks. Drains only
        // scheduler-owned tasks from the shared registry so hooks for
        // other subsystems don't see them.
        ctx.on_serve(move |serve_ctx| {
            let tasks = serve_ctx.task_registry().take_of::<ScheduledTaskMarker>();
            start_scheduled_tasks(tasks, token_for_serve, job_registry_for_serve);
        });

        // Register shutdown hook.
        ctx.on_shutdown(move || {
            cancel_for_stopper.cancel();
        });

        (token, job_registry)
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
        AppBuilderSchedulerExt, ScheduleConfig, ScheduledJobInfo, ScheduledJobRegistry,
        ScheduledTaskDef, Scheduler, SchedulerHandle,
    };
}

/// Format a schedule config as a human-readable string.
fn format_schedule(config: &ScheduleConfig) -> String {
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
///
/// Tasks already have their state captured (via `ScheduledTaskDef.state`), so
/// no state parameter is needed here. The function extracts tasks by downcasting
/// to `Box<dyn ScheduledTask>`, populates the job registry with metadata, and starts them.
fn start_scheduled_tasks(
    boxed_tasks: Vec<Box<dyn Any + Send>>,
    token: CancellationToken,
    job_registry: ScheduledJobRegistry,
) {
    let tasks = extract_tasks(boxed_tasks);
    if tasks.is_empty() {
        return;
    }

    // Populate the job registry with task metadata before starting.
    for task in &tasks {
        job_registry.register(ScheduledJobInfo {
            name: task.name().to_string(),
            schedule: format_schedule(task.schedule()),
        });
    }

    tracing::info!(count = tasks.len(), "Starting scheduled tasks");
    for task in tasks {
        task.start(token.clone());
    }
}
