//! Scheduling types and traits.
//!
//! These types define how scheduled tasks are configured and executed.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// What the scheduler does when a job's own tick is still running as its next
/// fire time arrives.
///
/// The policy is per-job and only concerns a job overlapping *with itself*;
/// different jobs always run concurrently regardless.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlapPolicy {
    /// Never run a job concurrently with itself: the next tick is armed only
    /// when the running one completes. A fire that comes due while the previous
    /// tick is still in flight is skipped (cadence is preserved — the schedule
    /// is advanced, not stalled). This is the default.
    #[default]
    Skip,
    /// Let a job overlap with itself: the next tick is armed at *fire* time, so
    /// a slow tick does not hold back the following one. Ticks pile up under
    /// sustained load. Interval cadence stays anchored; cron recomputes the next
    /// fire when the job fires.
    Concurrent,
}

/// How a scheduled task should be triggered.
pub enum ScheduleConfig {
    /// Run at a fixed interval (e.g., every 60 seconds).
    Interval(crate::duration::PositiveDuration),
    /// Run at a fixed interval with an initial delay before the first execution.
    IntervalWithDelay {
        interval: crate::duration::PositiveDuration,
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
                message:
                    "empty schedule string (expected a duration like \"30s\" or a cron expression)"
                        .to_string(),
            });
        }

        if s.contains(char::is_whitespace) || s.starts_with('@') {
            s.parse::<cron::Schedule>()
                .map_err(|e| ScheduleParseError {
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
            ConfigValue::String(s) => {
                s.parse()
                    .map_err(|e: ScheduleParseError| ConfigError::Deserialize {
                        key: key.to_string(),
                        message: e.message,
                    })
            }
            ConfigValue::Integer(i) if *i > 0 => Ok(ScheduleConfig::Interval(
                crate::duration::PositiveDuration::from_secs(*i as u64)
                    .expect("a strictly positive integer is a positive duration"),
            )),
            _ => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected:
                    "schedule string (duration or cron expression) or positive integer seconds",
            }),
        }
    }
}

/// A skip predicate evaluated at the start of each tick — the counterpart of
/// Quarkus' `skipExecutionIf`. Returning `true` suppresses the tick body: the
/// schedule keeps advancing, the skip is recorded in
/// [`ScheduledJobInfo::skip_count`](crate::ScheduledJobInfo), and nothing runs.
pub type SkipFn = Box<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// A runnable schedule handed to the driver.
///
/// Produced by [`ScheduledTask::into_job`]. The task's state is already
/// captured inside `run` (a clone happens per tick — the documented contract),
/// so the driver only needs the name, the schedule, and a nullary factory that
/// produces one tick future per fire.
pub struct ScheduledJob {
    /// The name of this task (for logging and the job registry).
    pub name: String,
    /// The schedule configuration.
    pub schedule: ScheduleConfig,
    /// How this job behaves when a tick is still running as the next fire is due.
    pub overlap: OverlapPolicy,
    /// Produces one tick future each time the job fires.
    pub run: Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    /// Optional skip predicate, checked at the start of every tick (scheduled
    /// and trigger-now alike). `true` = skip this tick's body.
    pub skip: Option<SkipFn>,
}

/// A scheduled task that can be converted into a driver [`ScheduledJob`].
///
/// This trait uses `self: Box<Self>` to allow the task to take ownership
/// of captured state. No generic type T appears in the trait methods,
/// making it object-safe.
///
/// The `'static` bound is required so that `Box<dyn ScheduledTask>` can be
/// stored as `Box<dyn Any>` for type erasure in the core crate.
///
/// All schedules are driven by a single driver task (see
/// [`start_jobs`](crate::start_jobs)); tick bodies are submitted to the shared
/// [`PoolExecutor`](r2e_executor::PoolExecutor) rather than run inline, so a
/// panicking tick is contained in its pool job and the driver keeps ticking.
pub trait ScheduledTask: Send + 'static {
    /// The name of this task (for logging).
    fn name(&self) -> &str;

    /// The schedule configuration.
    fn schedule(&self) -> &ScheduleConfig;

    /// Convert into a [`ScheduledJob`] the driver can run.
    fn into_job(self: Box<Self>) -> ScheduledJob;
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
    /// Self-overlap policy for this task. Defaults to [`OverlapPolicy::Skip`]
    /// (via [`new`](Self::new) / [`from_fn`](Self::from_fn)); set it with
    /// [`with_overlap`](Self::with_overlap).
    pub overlap: OverlapPolicy,
    pub state: T,
    pub task: Box<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    /// Optional skip predicate (Quarkus `skipExecutionIf`), receiving a clone
    /// of the state on every fire. `None` (via [`new`](Self::new) /
    /// [`from_fn`](Self::from_fn)) means never skip; set it with
    /// [`with_skip_if`](Self::with_skip_if).
    pub skip: Option<Box<dyn Fn(T) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>>,
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
    pub fn new<F, Fut>(name: impl Into<String>, schedule: ScheduleConfig, state: T, task: F) -> Self
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
            overlap: OverlapPolicy::Skip,
            state,
            task: Box::new(move |state| {
                let fut = task(state);
                let task_name = task_name.clone();
                Box::pin(async move { fut.await.log_if_err(&task_name) })
            }),
            skip: None,
        }
    }

    /// Set this task's self-overlap policy (default [`OverlapPolicy::Skip`]).
    ///
    /// ```ignore
    /// ScheduledTaskDef::from_fn("poll", "50ms".parse()?, || async { work().await })
    ///     .with_overlap(OverlapPolicy::Concurrent);
    /// ```
    pub fn with_overlap(mut self, overlap: OverlapPolicy) -> Self {
        self.overlap = overlap;
        self
    }

    /// Set this task's skip predicate — the dynamic-task counterpart of
    /// `#[scheduled(skip_if = "...")]` (Quarkus `skipExecutionIf`).
    ///
    /// The predicate receives a clone of the state at every fire (scheduled
    /// and `trigger_now` alike) and runs before the task body; returning
    /// `true` skips the tick. Skips are counted in
    /// [`ScheduledJobInfo::skip_count`](crate::ScheduledJobInfo) and the
    /// schedule keeps advancing.
    ///
    /// ```ignore
    /// ScheduledTaskDef::new("sync", "5m".parse()?, svc, |svc| async move { svc.sync().await })
    ///     .with_skip_if(|svc| async move { svc.maintenance_mode().await });
    /// ```
    pub fn with_skip_if<F, Fut>(mut self, pred: F) -> Self
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        self.skip = Some(Box::new(move |state| Box::pin(pred(state))));
        self
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

    fn into_job(self: Box<Self>) -> ScheduledJob {
        // Move state and the task closure into a nullary factory. For
        // `#[scheduled]` macro tasks `state` is `()` so this clone is free; for
        // dynamic tasks with real state, the per-tick clone is the existing
        // documented contract.
        let state = self.state;
        let task = self.task;
        let skip = self.skip.map(|skip| {
            let state = state.clone();
            Box::new(move || skip(state.clone())) as SkipFn
        });
        ScheduledJob {
            name: self.name,
            schedule: self.schedule,
            overlap: self.overlap,
            run: Box::new(move || task(state.clone())),
            skip,
        }
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
