//! Firing a `#[scheduled]` method of the real app from a test.
//!
//! A test boot starts no scheduler driver, so nothing ticks on a timer;
//! `app.scheduler().run("job")` runs one tick on the executor pool and returns
//! when it has finished (feature `scheduler-testing`).

use r2e::r2e_scheduler::testing::{SchedulerTestExt, TickOutcome};
use r2e_test::TestApp;

#[r2e::test(app = example_app::ExampleApp)]
async fn a_scheduled_method_of_the_app_runs_on_demand(app: TestApp) {
    let scheduler = app.scheduler();

    // `ScheduledJobs::count_users` — the default job name is `{Type}_{method}`.
    let job = "ScheduledJobs_count_users";
    assert_eq!(scheduler.job(job).expect("job registered").run_count, 0);

    assert_eq!(scheduler.run(job).await, TickOutcome::Ran);

    let info = scheduler.job(job).unwrap();
    assert_eq!(info.run_count, 1);
    assert!(info.last_duration.is_some());
}
