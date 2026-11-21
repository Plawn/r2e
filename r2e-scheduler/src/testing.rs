//! Test helpers for apps with scheduled jobs (feature `testing`).
//!
//! A test boot ([`TestApp::boot`]) starts the app in process and never runs
//! the scheduler driver: no job ticks on its own, so a test is never raced by
//! a timer it did not ask for. [`SchedulerTestExt`] adds `app.scheduler()` to
//! fire a job by name and wait for it:
//!
//! ```ignore
//! use r2e_scheduler::testing::{SchedulerTestExt, TickOutcome};
//!
//! let app = TestApp::boot::<MyApp>().await;
//! assert_eq!(app.scheduler().run("CleanupJobs_purge_expired").await, TickOutcome::Ran);
//! app.get("/sessions").send().await.assert_json_path("len()", 0);
//! ```
//!
//! The tick is the one the driver submits in production — the `skip_if`
//! predicate, then the body, as one job of the executor pool — and the job's
//! [`ScheduledJobInfo`] stats move the same way. Only the cadence is absent:
//! a job runs when the test says so, and at no other time.

use std::sync::Arc;

use r2e_test::TestApp;

pub use crate::manual::TickOutcome;
use crate::manual::{ManualJobs, Unavailable};
use crate::{ScheduledJobInfo, ScheduledJobRegistry};

/// `app.scheduler()` on a [`TestApp`].
pub trait SchedulerTestExt {
    /// The scheduled jobs of the app, to fire by hand.
    ///
    /// # Panics
    ///
    /// Panics if the app has no enabled [`Scheduler`](crate::Scheduler) plugin
    /// (not installed, or `scheduler.enabled = false`).
    fn scheduler(&self) -> TestScheduler;
}

impl SchedulerTestExt for TestApp {
    fn scheduler(&self) -> TestScheduler {
        let registry = self.bean::<ScheduledJobRegistry>();
        match registry.manual_jobs() {
            Ok(jobs) => TestScheduler { jobs, registry },
            Err(Unavailable::NotInstalled) => panic!(
                "this app has no scheduled jobs to run: the `Scheduler` plugin is disabled \
                 (`scheduler.enabled = false`) or the `ScheduledJobRegistry` bean was not \
                 provided by it"
            ),
            Err(Unavailable::Driven) => panic!(
                "this app is served: its scheduler driver owns the jobs — use \
                 `SchedulerHandle::trigger_now` instead"
            ),
        }
    }
}

/// The scheduled jobs of a [`TestApp`], from [`SchedulerTestExt::scheduler`].
///
/// Covers every job the app registered: `#[scheduled]` methods on beans and
/// controllers, and dynamic tasks
/// ([`schedule_task`](crate::AppBuilderSchedulerExt::schedule_task)).
#[derive(Clone)]
pub struct TestScheduler {
    jobs: Arc<ManualJobs>,
    registry: ScheduledJobRegistry,
}

impl TestScheduler {
    /// Run one tick of the job `name` and wait for it to finish.
    ///
    /// A `#[scheduled]` method is named `{Type}_{method}` unless the attribute
    /// sets `name = "..."`. Returns [`TickOutcome::Skipped`] when the job's
    /// `skip_if` predicate suppressed the body.
    ///
    /// A job returning `Err` logs it and still counts as [`TickOutcome::Ran`],
    /// as in production — assert on what the job did, not on its return value.
    ///
    /// # Panics
    ///
    /// Panics if no job has that name (the message lists the ones that exist),
    /// and re-raises a panic of the job itself, so a job that panics fails the
    /// test that ran it.
    pub async fn run(&self, name: &str) -> TickOutcome {
        match self.jobs.run(name).await {
            Some(outcome) => outcome,
            None => panic!(
                "no scheduled job named `{name}` — registered jobs: {:?}",
                self.jobs.names()
            ),
        }
    }

    /// Every registered job with its current stats.
    pub fn jobs(&self) -> Vec<ScheduledJobInfo> {
        self.registry.list_jobs()
    }

    /// One job with its current stats (`run_count`, `skip_count`, `last_run`,
    /// …), or `None` if no job has that name.
    pub fn job(&self, name: &str) -> Option<ScheduledJobInfo> {
        self.registry.job(name)
    }
}
