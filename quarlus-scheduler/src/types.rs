//! Scheduling types and traits.
//!
//! These types define how scheduled tasks are configured and executed.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How a scheduled task should be triggered.
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

/// A scheduled task that can be started.
///
/// This trait uses `self: Box<Self>` to allow the task to take ownership
/// of captured state. No generic type T appears in the trait methods,
/// making it object-safe.
///
/// The `'static` bound is required so that `Box<dyn ScheduledTask>` can be
/// stored as `Box<dyn Any>` for type erasure in the core crate.
pub trait ScheduledTask: Send + 'static {
    /// The name of this task (for logging).
    fn name(&self) -> &str;

    /// The schedule configuration.
    fn schedule(&self) -> &ScheduleConfig;

    /// Start the task with the given cancellation token.
    ///
    /// This spawns a Tokio task that runs according to the schedule
    /// until cancellation is requested.
    fn start(self: Box<Self>, cancel: CancellationToken);
}

/// Extract scheduled tasks from type-erased boxes.
///
/// Tasks are stored as `Box<Box<dyn ScheduledTask + Send>>` wrapped in `Box<dyn Any + Send>`
/// to keep quarlus-core scheduler-agnostic. This function extracts them back to
/// trait objects that can be started.
pub fn extract_tasks(boxed: Vec<Box<dyn Any + Send>>) -> Vec<Box<dyn ScheduledTask>> {
    boxed
        .into_iter()
        .filter_map(|b| {
            b.downcast::<Box<dyn ScheduledTask>>()
                .ok()
                .map(|inner| *inner)
        })
        .collect()
}

/// A scheduled task definition that captures state.
///
/// This struct owns a clone of the application state and the task closure.
/// When `start` is called, it has everything it needs to run the task
/// without needing to receive the state.
pub struct ScheduledTaskDef<T: Clone + Send + Sync + 'static> {
    pub name: String,
    pub schedule: ScheduleConfig,
    pub state: T,
    pub task: Box<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
}

impl<T: Clone + Send + Sync + 'static> ScheduledTask for ScheduledTaskDef<T> {
    fn name(&self) -> &str {
        &self.name
    }

    fn schedule(&self) -> &ScheduleConfig {
        &self.schedule
    }

    fn start(self: Box<Self>, cancel: CancellationToken) {
        let name = self.name;
        let schedule = self.schedule;
        let state = self.state;
        let task = self.task;

        tokio::spawn(async move {
            tracing::info!(task = %name, "Scheduled task started");
            match schedule {
                ScheduleConfig::Interval(interval) => {
                    run_interval(&name, interval, Duration::ZERO, state, cancel, &*task).await;
                }
                ScheduleConfig::IntervalWithDelay { interval, initial_delay } => {
                    run_interval(&name, interval, initial_delay, state, cancel, &*task).await;
                }
                ScheduleConfig::Cron(expr) => {
                    run_cron(&name, &expr, state, cancel, &*task).await;
                }
            }
            tracing::info!(task = %name, "Scheduled task stopped");
        });
    }
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

async fn run_interval<T: Clone + Send + Sync + 'static>(
    name: &str,
    interval: Duration,
    initial_delay: Duration,
    state: T,
    cancel: CancellationToken,
    task: &(dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync),
) {
    if !initial_delay.is_zero() {
        tokio::select! {
            _ = tokio::time::sleep(initial_delay) => {},
            _ = cancel.cancelled() => { return; }
        }
    }

    let mut tick = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(task = %name, "Executing scheduled task");
                task(state.clone()).await;
            }
            _ = cancel.cancelled() => {
                break;
            }
        }
    }
}

async fn run_cron<T: Clone + Send + Sync + 'static>(
    name: &str,
    expr: &str,
    state: T,
    cancel: CancellationToken,
    task: &(dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync),
) {
    let schedule = match expr.parse::<cron::Schedule>() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(task = %name, error = %e, "Invalid cron expression");
            return;
        }
    };

    loop {
        let now = chrono::Utc::now();
        let next = match schedule.upcoming(chrono::Utc).next() {
            Some(n) => n,
            None => {
                tracing::warn!(task = %name, "No more upcoming cron executions");
                break;
            }
        };

        let until = (next - now).to_std().unwrap_or(Duration::from_secs(1));

        tokio::select! {
            _ = tokio::time::sleep(until) => {
                tracing::debug!(task = %name, "Executing cron task");
                task(state.clone()).await;
            }
            _ = cancel.cancelled() => {
                break;
            }
        }
    }
}
