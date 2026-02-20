//! Background task scheduler for R2E.
//!
//! Provides interval, cron, and delayed task execution. Install with
//! `.plugin(Scheduler)` before `build_state()`.

mod types;

pub use types::{extract_tasks, ScheduleConfig, ScheduledResult, ScheduledTask, ScheduledTaskDef};

use std::any::Any;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use r2e_core::builder::TaskRegistryHandle;
use r2e_core::http::StatusCode;
use r2e_core::type_list::{TAppend, TCons, TNil};
use r2e_core::{AppBuilder, DeferredAction, RawPreStatePlugin};

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
/// #[derive(Controller)]
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
/// This plugin uses the [`RawPreStatePlugin`] API because it provides
/// **two** beans (`CancellationToken` and `ScheduledJobRegistry`).
///
/// # Example
///
/// ```ignore
/// use r2e_scheduler::Scheduler;
///
/// AppBuilder::new()
///     .plugin(Scheduler)  // Before build_state()!
///     .build_state::<Services, _, _>()
///     .await
///     .register_controller::<ScheduledJobs>()
///     .serve("0.0.0.0:3000")
/// ```
///
/// In controllers, inject the token or job registry directly:
///
/// ```ignore
/// #[derive(Controller)]
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

impl RawPreStatePlugin for Scheduler {
    type Provisions = TCons<CancellationToken, TCons<ScheduledJobRegistry, TNil>>;
    type Required = TNil;

    fn install<P, R>(
        self,
        app: AppBuilder<r2e_core::builder::NoState, P, R>,
    ) -> AppBuilder<r2e_core::builder::NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let token = CancellationToken::new();
        let job_registry = ScheduledJobRegistry::new();
        let handle = SchedulerHandle::new(token.clone());
        let task_registry = TaskRegistryHandle::new();
        let cancel_for_stopper = token.clone();
        let token_for_serve = token.clone();
        let job_registry_for_serve = job_registry.clone();

        app.provide(token)
            .provide(job_registry)
            .add_deferred(DeferredAction::new("Scheduler", move |ctx| {
                // Add the layer that provides SchedulerHandle via extension.
                ctx.add_layer(Box::new(move |router| {
                    router.layer(axum::Extension(handle))
                }));

                // Store the task registry for use during controller registration.
                ctx.store_data(task_registry);

                // Register a serve hook to start scheduled tasks.
                // Tasks already have their state captured, so no generic T is needed.
                ctx.on_serve(move |tasks, _token_at_serve| {
                    start_scheduled_tasks(tasks, token_for_serve, job_registry_for_serve);
                });

                // Register shutdown hook.
                ctx.on_shutdown(move || {
                    cancel_for_stopper.cancel();
                });
            }))
            .with_updated_types()
    }
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
