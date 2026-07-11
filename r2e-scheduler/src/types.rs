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

/// Error returned when a schedule string cannot be parsed.
#[derive(Debug, Clone)]
pub struct ScheduleParseError {
    message: String,
}

impl std::fmt::Display for ScheduleParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScheduleParseError {}

/// Parse a schedule from a string — the config-driven counterpart of
/// `#[scheduled(every = "...")]` / `#[scheduled(cron = "...")]`.
///
/// - A duration string (`"30s"`, `"5m"`, `"1h30m"`) becomes [`ScheduleConfig::Interval`].
/// - A cron expression (contains whitespace, or starts with `@` like `"@hourly"`)
///   is validated and becomes [`ScheduleConfig::Cron`].
///
/// ```ignore
/// let every: ScheduleConfig = "30s".parse()?;
/// let nightly: ScheduleConfig = "0 0 2 * * *".parse()?;
/// ```
impl std::str::FromStr for ScheduleConfig {
    type Err = ScheduleParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ScheduleParseError {
                message: "empty schedule string (expected a duration like \"30s\" or a cron expression)"
                    .to_string(),
            });
        }

        if s.contains(char::is_whitespace) || s.starts_with('@') {
            s.parse::<cron::Schedule>().map_err(|e| ScheduleParseError {
                message: format!("invalid cron expression '{}': {}", s, e),
            })?;
            Ok(ScheduleConfig::Cron(s.to_string()))
        } else {
            crate::duration::parse_duration(s)
                .map(ScheduleConfig::Interval)
                .map_err(|e| ScheduleParseError {
                    message: format!(
                        "invalid schedule '{}': {} (expected a duration like \"30s\" or a cron expression)",
                        s, e
                    ),
                })
        }
    }
}

/// Read a schedule from configuration, enabling
/// `#[config("app.sync.schedule")] schedule: ScheduleConfig` and
/// config-driven dynamic task registration.
///
/// Accepts a string (duration or cron, see the [`FromStr`](#impl-FromStr-for-ScheduleConfig)
/// impl) or an integer interpreted as seconds — mirroring `#[scheduled(every = 30)]`.
impl r2e_core::config::FromConfigValue for ScheduleConfig {
    fn from_config_value(
        value: &r2e_core::config::ConfigValue,
        key: &str,
    ) -> Result<Self, r2e_core::config::ConfigError> {
        use r2e_core::config::{ConfigError, ConfigValue};
        match value {
            ConfigValue::String(s) => s.parse().map_err(|e: ScheduleParseError| {
                ConfigError::Deserialize {
                    key: key.to_string(),
                    message: e.message,
                }
            }),
            ConfigValue::Integer(i) if *i > 0 => {
                Ok(ScheduleConfig::Interval(Duration::from_secs(*i as u64)))
            }
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "schedule string (duration or cron expression) or positive integer seconds",
            }),
        }
    }
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
/// to keep r2e-core scheduler-agnostic. This function extracts them back to
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

impl<T: Clone + Send + Sync + 'static> ScheduledTaskDef<T> {
    /// Create a task definition from a name, a schedule, captured state, and
    /// an async closure receiving a clone of the state on every tick.
    ///
    /// The closure may return `()` or `Result<(), E: Display>` — errors are
    /// logged under the task name (same contract as `#[scheduled]` methods).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let task = ScheduledTaskDef::new(
    ///     "sync_source",
    ///     "5m".parse()?,
    ///     source_service.clone(),
    ///     |svc| async move { svc.sync().await },
    /// );
    /// ```
    pub fn new<F, Fut>(
        name: impl Into<String>,
        schedule: ScheduleConfig,
        state: T,
        task: F,
    ) -> Self
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: ScheduledResult,
    {
        let name = name.into();
        let task_name = name.clone();
        Self {
            name,
            schedule,
            state,
            task: Box::new(move |state| {
                let fut = task(state);
                let task_name = task_name.clone();
                Box::pin(async move { fut.await.log_if_err(&task_name) })
            }),
        }
    }

    /// Type-erase this definition into the `Box<dyn Any + Send>` shape stored
    /// in the core task registry (`Box<Box<dyn ScheduledTask>>` internally —
    /// the counterpart of [`extract_tasks`]).
    ///
    /// Most callers should use `AppBuilderSchedulerExt::schedule_task`
    /// instead, which does this and the registry insertion in one step.
    pub fn into_boxed_any(self) -> Box<dyn Any + Send> {
        let boxed: Box<dyn ScheduledTask> = Box::new(self);
        Box::new(boxed)
    }
}

impl ScheduledTaskDef<()> {
    /// Create a stateless task definition from a name, a schedule, and an
    /// async closure. State the task needs should be moved into the closure.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let task = ScheduledTaskDef::from_fn("heartbeat", "30s".parse()?, || async {
    ///     tracing::info!("still alive");
    /// });
    /// ```
    pub fn from_fn<F, Fut>(name: impl Into<String>, schedule: ScheduleConfig, task: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: ScheduledResult,
    {
        Self::new(name, schedule, (), move |()| task())
    }
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

        r2e_core::rt::spawn(async move {
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
            _ = r2e_core::rt::sleep(initial_delay) => {},
            _ = cancel.cancelled() => { return; }
        }
    }

    let mut tick = r2e_core::rt::interval(interval);
    tick.set_missed_tick_behavior(r2e_core::rt::MissedTickBehavior::Skip);
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
            _ = r2e_core::rt::sleep(until) => {
                tracing::debug!(task = %name, "Executing cron task");
                task(state.clone()).await;
            }
            _ = cancel.cancelled() => {
                break;
            }
        }
    }
}
