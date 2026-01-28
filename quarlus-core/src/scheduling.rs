use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// How a scheduled task should be triggered.
///
/// This is a pure data type living in `quarlus-core` so that the macro crate
/// can reference it without depending on `quarlus-scheduler`. The scheduler
/// runtime converts this into its internal `Schedule` enum.
pub enum ScheduleConfig {
    /// Run at a fixed interval (e.g., every 60 seconds).
    Interval(Duration),
    /// Run at a fixed interval with an initial delay before the first execution.
    IntervalWithDelay {
        interval: Duration,
        initial_delay: Duration,
    },
    /// Run on a cron expression (e.g., `"0 */5 * * * *"` = every 5 minutes).
    Cron(String),
}

/// A single scheduled task definition collected from a controller.
///
/// The `task` closure receives the application state and returns a future.
/// The scheduler runtime is responsible for invoking it on the configured schedule.
pub struct ScheduledTaskDef<T: Clone + Send + Sync + 'static> {
    pub name: String,
    pub schedule: ScheduleConfig,
    pub task: Box<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
}

/// Trait for handling scheduled task return values.
///
/// Allows scheduled methods to return `()` (infallible) or `Result<(), E>`
/// (logs on error). The proc-macro wraps every scheduled call with
/// `ScheduledResult::log_if_err(result, task_name)`.
pub trait ScheduledResult {
    fn log_if_err(self, task_name: &str);
}

impl ScheduledResult for () {
    fn log_if_err(self, _: &str) {}
}

impl<E: std::fmt::Display> ScheduledResult for Result<(), E> {
    fn log_if_err(self, task_name: &str) {
        if let Err(e) = self {
            tracing::error!(task = %task_name, error = %e, "Scheduled task failed");
        }
    }
}

/// Type-erased function that starts all scheduled tasks given the state.
pub type SchedulerStartFn<T> = Box<
    dyn FnOnce(Vec<ScheduledTaskDef<T>>, T) + Send,
>;

/// Type-erased function that stops the scheduler.
pub type SchedulerStopFn = Box<dyn FnOnce() + Send>;
