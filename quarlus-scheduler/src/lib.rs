use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use quarlus_core::scheduling::{ScheduleConfig, ScheduledTaskDef};

/// Scheduler plugin — installs the scheduler runtime into the application.
///
/// Add `.with(Scheduler)` to your `AppBuilder` to enable scheduled tasks.
/// Controllers that declare `#[scheduled]` methods are auto-discovered via
/// `register_controller()`.
///
/// # Example
///
/// ```ignore
/// use quarlus_scheduler::Scheduler;
///
/// AppBuilder::new()
///     .build_state::<Services>()
///     .with(Scheduler)
///     .register_controller::<ScheduledJobs>()
///     .serve("0.0.0.0:3000")
/// ```
pub struct Scheduler;

impl<T: Clone + Send + Sync + 'static> quarlus_core::Plugin<T> for Scheduler {
    fn install(self, app: quarlus_core::AppBuilder<T>) -> quarlus_core::AppBuilder<T> {
        let cancel = CancellationToken::new();
        let cancel_stop = cancel.clone();

        app.set_scheduler_backend(
            Box::new(move |task_defs, state| {
                start_tasks_from_defs(task_defs, state, cancel);
            }),
            Box::new(move || {
                cancel_stop.cancel();
            }),
        )
    }
}

/// Convert [`ScheduledTaskDef`]s (from quarlus-core) into running Tokio tasks.
fn start_tasks_from_defs<T: Clone + Send + Sync + 'static>(
    task_defs: Vec<ScheduledTaskDef<T>>,
    state: T,
    cancel: CancellationToken,
) {
    for def in task_defs {
        let state = state.clone();
        let cancel = cancel.clone();
        let name = def.name.clone();

        tokio::spawn(async move {
            tracing::info!(task = %name, "Scheduled task started");
            match def.schedule {
                ScheduleConfig::Interval(interval) => {
                    run_interval(&name, interval, Duration::ZERO, state, cancel, &def.task)
                        .await;
                }
                ScheduleConfig::IntervalWithDelay {
                    interval,
                    initial_delay,
                } => {
                    run_interval(&name, interval, initial_delay, state, cancel, &def.task)
                        .await;
                }
                ScheduleConfig::Cron(expr) => {
                    run_cron(&name, &expr, state, cancel, &def.task).await;
                }
            }
            tracing::info!(task = %name, "Scheduled task stopped");
        });
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
