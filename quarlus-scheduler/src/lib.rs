use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How a task should be scheduled.
pub enum Schedule {
    /// Run at a fixed interval (e.g., every 60 seconds).
    Every(Duration),
    /// Run at a fixed interval with an initial delay before the first execution.
    EveryDelay { interval: Duration, initial_delay: Duration },
    /// Run on a cron expression (e.g., `"0 */5 * * * *"` = every 5 minutes).
    Cron(String),
}

/// A single scheduled task.
pub struct ScheduledTask<T: Clone + Send + Sync + 'static> {
    pub name: String,
    pub schedule: Schedule,
    pub task: Box<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
}

/// A scheduler that manages a collection of background tasks.
///
/// Tasks run until the provided `CancellationToken` is cancelled.
pub struct Scheduler<T: Clone + Send + Sync + 'static> {
    tasks: Vec<ScheduledTask<T>>,
}

impl<T: Clone + Send + Sync + 'static> Scheduler<T> {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Add a scheduled task.
    pub fn add_task(&mut self, task: ScheduledTask<T>) {
        self.tasks.push(task);
    }

    /// Start all scheduled tasks as background Tokio tasks.
    ///
    /// Tasks will keep running until the `cancel` token is cancelled.
    pub fn start(self, state: T, cancel: CancellationToken) {
        for task in self.tasks {
            let state = state.clone();
            let cancel = cancel.clone();
            let name = task.name.clone();

            tokio::spawn(async move {
                tracing::info!(task = %name, "Scheduled task started");
                match task.schedule {
                    Schedule::Every(interval) => {
                        run_interval(&name, interval, Duration::ZERO, state, cancel, &task.task).await;
                    }
                    Schedule::EveryDelay { interval, initial_delay } => {
                        run_interval(&name, interval, initial_delay, state, cancel, &task.task).await;
                    }
                    Schedule::Cron(expr) => {
                        run_cron(&name, &expr, state, cancel, &task.task).await;
                    }
                }
                tracing::info!(task = %name, "Scheduled task stopped");
            });
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Default for Scheduler<T> {
    fn default() -> Self {
        Self::new()
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
