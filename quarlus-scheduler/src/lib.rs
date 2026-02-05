//! Background task scheduler for Quarlus.
//!
//! Provides interval, cron, and delayed task execution. Install with
//! `.with_plugin(Scheduler)` before `build_state()`.

mod types;

pub use types::{extract_tasks, ScheduleConfig, ScheduledResult, ScheduledTask, ScheduledTaskDef};

use std::any::Any;
use std::future::Future;
use tokio_util::sync::CancellationToken;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use quarlus_core::builder::TaskRegistryHandle;
use quarlus_core::http::StatusCode;
use quarlus_core::type_list::TCons;
use quarlus_core::{AppBuilder, DeferredAction, PreStatePlugin};

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
                    "Scheduler not installed. Add `.with_plugin(Scheduler)` before build_state().",
                ))
        }
    }
}

/// Scheduler plugin — provides `CancellationToken` and installs the scheduler runtime.
///
/// Install this plugin **before** `build_state()` to:
/// - Provide a `CancellationToken` bean for injection via `#[inject]`
/// - Automatically set up the scheduler backend
///
/// # Example
///
/// ```ignore
/// use quarlus_scheduler::Scheduler;
///
/// AppBuilder::new()
///     .with_plugin(Scheduler)  // Before build_state()!
///     .build_state::<Services, _>()
///     .register_controller::<ScheduledJobs>()
///     .serve("0.0.0.0:3000")
/// ```
///
/// In controllers, inject the token directly:
///
/// ```ignore
/// #[derive(Controller)]
/// #[controller(state = Services)]
/// pub struct MyController {
///     #[inject] cancel: CancellationToken,
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
    type Provided = CancellationToken;

    fn install<P>(
        self,
        app: AppBuilder<quarlus_core::builder::NoState, P>,
    ) -> AppBuilder<quarlus_core::builder::NoState, TCons<Self::Provided, P>> {
        let token = CancellationToken::new();
        let handle = SchedulerHandle::new(token.clone());
        let registry = TaskRegistryHandle::new();
        let cancel_for_stopper = token.clone();
        let token_for_serve = token.clone();

        app.provide(token).add_deferred(DeferredAction::new("Scheduler", move |ctx| {
            // Add the layer that provides SchedulerHandle via extension.
            ctx.add_layer(Box::new(move |router| {
                router.layer(axum::Extension(handle))
            }));

            // Store the task registry for use during controller registration.
            ctx.store_data(registry);

            // Register a serve hook to start scheduled tasks.
            // Tasks already have their state captured, so no generic T is needed.
            ctx.on_serve(move |tasks, _token_at_serve| {
                start_scheduled_tasks(tasks, token_for_serve);
            });

            // Register shutdown hook.
            ctx.on_shutdown(move || {
                cancel_for_stopper.cancel();
            });
        }))
    }
}

/// Start scheduled tasks from boxed task definitions.
///
/// This function is called by the builder's serve() method. It receives:
/// - `boxed_tasks`: Type-erased task definitions (Vec<Box<dyn Any + Send>>)
/// - `token`: The cancellation token
///
/// Tasks already have their state captured (via `ScheduledTaskDef.state`), so
/// no state parameter is needed here. The function extracts tasks by downcasting
/// to `Box<dyn ScheduledTask>` and starts them.
fn start_scheduled_tasks(
    boxed_tasks: Vec<Box<dyn Any + Send>>,
    token: CancellationToken,
) {
    let tasks = extract_tasks(boxed_tasks);
    if tasks.is_empty() {
        return;
    }

    tracing::info!(count = tasks.len(), "Starting scheduled tasks");
    for task in tasks {
        task.start(token.clone());
    }
}

