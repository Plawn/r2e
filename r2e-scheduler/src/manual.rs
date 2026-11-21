//! Firing a job by hand, with no driver.
//!
//! An in-process start (`TestApp::boot`) never runs the serve hook that starts
//! the driver, so the registered `#[scheduled]` jobs would otherwise be
//! unreachable: nothing ticks them, and the [`SchedulerHandle`](crate::SchedulerHandle)
//! has no driver to answer `trigger_now`. The plugin therefore leaves what the
//! driver would have consumed — the collected tasks and the pool ticks run on
//! — on the [`ScheduledJobRegistry`] bean, where the `testing` feature picks it
//! up to run one tick at a time, awaited to completion.
//!
//! A served app is unaffected: its serve hook marks the jobs as driven before
//! it drains them, and nothing here is reachable afterwards.

#![cfg_attr(
    not(feature = "testing"),
    expect(
        dead_code,
        reason = "the plugin always leaves the jobs here; only the `testing` feature reads them"
    )
)]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_executor::PoolExecutor;

use crate::types::{extract_tasks, ScheduledJob};
use crate::{format_schedule, ScheduledJobInfo, ScheduledJobRegistry};

/// What happened to a tick fired by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// The job's body ran to completion.
    Ran,
    /// The job's `skip_if` predicate suppressed the body.
    Skipped,
}

/// Why the jobs of an app cannot be fired by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// No enabled [`Scheduler`](crate::Scheduler) plugin left its jobs here.
    NotInstalled,
    /// The app is served: the driver owns the jobs.
    Driven,
}

/// Where the jobs of an app are, from the point of view of a manual fire.
#[derive(Default)]
pub(crate) enum ManualState {
    #[default]
    NotInstalled,
    /// Collected but not converted yet: controllers keep registering tasks
    /// until the app is assembled, so the conversion waits for the first use.
    Dormant {
        tasks: TaskRegistryHandle,
        executor: PoolExecutor,
    },
    Ready(Arc<ManualJobs>),
    Driven,
}

/// The slot the plugin leaves on the registry bean.
#[derive(Clone, Default)]
pub(crate) struct ManualSlot(Arc<Mutex<ManualState>>);

impl ManualSlot {
    pub(crate) fn set(&self, state: ManualState) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }

    /// The jobs, converted on first use.
    pub(crate) fn jobs(
        &self,
        registry: &ScheduledJobRegistry,
    ) -> Result<Arc<ManualJobs>, Unavailable> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::take(&mut *state) {
            ManualState::NotInstalled => Err(Unavailable::NotInstalled),
            ManualState::Driven => {
                *state = ManualState::Driven;
                Err(Unavailable::Driven)
            }
            ManualState::Ready(jobs) => {
                *state = ManualState::Ready(Arc::clone(&jobs));
                Ok(jobs)
            }
            ManualState::Dormant { tasks, executor } => {
                let tasks = extract_tasks(tasks.take_of::<ScheduledTaskMarker>());
                // Same metadata the driver publishes when it starts.
                for task in &tasks {
                    registry.register(ScheduledJobInfo::new(
                        task.name().to_string(),
                        format_schedule(task.schedule()),
                    ));
                }
                let jobs = Arc::new(ManualJobs {
                    jobs: tasks.into_iter().map(|task| task.into_job()).collect(),
                    executor,
                    registry: registry.clone(),
                });
                *state = ManualState::Ready(Arc::clone(&jobs));
                Ok(jobs)
            }
        }
    }
}

/// The jobs of an app that no driver runs, ready to be fired one tick at a
/// time.
pub(crate) struct ManualJobs {
    jobs: Vec<ScheduledJob>,
    executor: PoolExecutor,
    registry: ScheduledJobRegistry,
}

impl ManualJobs {
    pub(crate) fn names(&self) -> Vec<&str> {
        self.jobs.iter().map(|job| job.name.as_str()).collect()
    }

    /// Run one tick of `name` on the pool and wait for it.
    ///
    /// The tick is what the driver would have submitted: the skip predicate,
    /// then the body, as one pool job reported under the task's name — and the
    /// job's stats move the way a driven tick moves them. `None` for an unknown
    /// name.
    ///
    /// # Panics
    ///
    /// Re-raises a panic of the job (body or skip predicate), and panics if the
    /// pool no longer accepts work.
    pub(crate) async fn run(&self, name: &str) -> Option<TickOutcome> {
        let job = self.jobs.iter().find(|job| job.name == name)?;
        let task: Arc<str> = Arc::from(name);
        let registry = self.registry.clone();
        let started = Instant::now();

        let skip = job.skip.as_ref().map(|skip| skip());
        let body = (job.run)();
        let tick = {
            let task = Arc::clone(&task);
            async move {
                if let Some(skip) = skip
                    && skip.await
                {
                    registry.update_job(&task, |info| info.skip_count += 1);
                    return TickOutcome::Skipped;
                }
                registry.update_job(&task, |info| {
                    info.last_run = Some(Utc::now());
                    info.run_count += 1;
                });
                body.await;
                TickOutcome::Ran
            }
        };

        let handle = self
            .executor
            .submit_scheduled(&task, tick)
            .unwrap_or_else(|_| {
                panic!("cannot run the scheduled job `{name}`: the executor pool is shut down")
            });
        match handle.await {
            Ok(outcome) => {
                if outcome == TickOutcome::Ran {
                    let elapsed = started.elapsed();
                    self.registry
                        .update_job(name, |info| info.last_duration = Some(elapsed));
                }
                Some(outcome)
            }
            Err(error) if error.is_panic() => {
                self.registry
                    .update_job(name, |info| info.panic_count += 1);
                std::panic::resume_unwind(error.into_panic())
            }
            Err(error) => panic!("the scheduled job `{name}` did not complete: {error}"),
        }
    }
}
