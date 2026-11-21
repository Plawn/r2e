//! `r2e_scheduler::testing` — firing the jobs of an in-process app by hand.
//!
//! Needs `--features testing` (the helpers pull `r2e-test`).
#![cfg(feature = "testing")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{App, AppBuilder, BootableApp};
use r2e_executor::Executor;
use r2e_scheduler::testing::{SchedulerTestExt, TickOutcome};
use r2e_scheduler::{
    AppBuilderSchedulerExt, PositiveDuration, ScheduleConfig, ScheduledTaskDef, Scheduler,
};
use r2e_test::TestApp;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the jobs of [`JobsApp`] report to, provided by each test.
#[derive(Clone, Default)]
struct Probe {
    ticks: Arc<AtomicUsize>,
    maintenance: Arc<AtomicBool>,
}

fn every(millis: u64) -> ScheduleConfig {
    ScheduleConfig::Interval(PositiveDuration::from_millis(millis).unwrap())
}

/// An app with three dynamic jobs: a counter, one gated by a skip predicate,
/// and one that panics.
struct JobsApp;

impl App for JobsApp {
    type Env = Probe;

    async fn setup() -> Result<Probe, BoxError> {
        Ok(Probe::default())
    }

    async fn build(b: AppBuilder, probe: Probe) -> Result<impl BootableApp, BoxError> {
        Ok(b
            .plugin(Executor)
            .plugin(Scheduler)
            .load_config::<()>()
            .build_state()
            .await
            .schedule_task(ScheduledTaskDef::new(
                "count",
                every(20),
                probe.clone(),
                |probe| async move {
                    probe.ticks.fetch_add(1, Ordering::SeqCst);
                },
            ))
            .schedule_task(
                ScheduledTaskDef::new("gated", every(20), probe.clone(), |probe| async move {
                    probe.ticks.fetch_add(10, Ordering::SeqCst);
                })
                .with_skip_if(|probe: Probe| async move {
                    probe.maintenance.load(Ordering::SeqCst)
                }),
            )
            .schedule_task(ScheduledTaskDef::from_fn("broken", every(20), blow_up)))
    }
}

async fn blow_up() {
    panic!("the job blew up");
}

async fn boot(probe: &Probe) -> TestApp {
    TestApp::boot_plain_env::<JobsApp>(probe.clone(), |b| b).await
}

#[r2e_core::test]
async fn run_fires_the_job_once_and_waits_for_it() {
    let probe = Probe::default();
    let app = boot(&probe).await;

    assert_eq!(app.scheduler().run("count").await, TickOutcome::Ran);
    // No polling: the body has finished when `run` returns.
    assert_eq!(probe.ticks.load(Ordering::SeqCst), 1);

    app.scheduler().run("count").await;
    assert_eq!(probe.ticks.load(Ordering::SeqCst), 2);
}

#[r2e_core::test]
async fn nothing_ticks_on_its_own() {
    let probe = Probe::default();
    let app = boot(&probe).await;
    let scheduler = app.scheduler();

    // Several 20ms periods go by.
    r2e_core::rt::sleep(Duration::from_millis(150)).await;

    assert_eq!(probe.ticks.load(Ordering::SeqCst), 0);
    assert_eq!(scheduler.job("count").unwrap().run_count, 0);
}

#[r2e_core::test]
async fn skip_predicate_is_evaluated_like_a_driven_tick() {
    let probe = Probe::default();
    let app = boot(&probe).await;
    let scheduler = app.scheduler();

    probe.maintenance.store(true, Ordering::SeqCst);
    assert_eq!(scheduler.run("gated").await, TickOutcome::Skipped);
    assert_eq!(probe.ticks.load(Ordering::SeqCst), 0);

    probe.maintenance.store(false, Ordering::SeqCst);
    assert_eq!(scheduler.run("gated").await, TickOutcome::Ran);
    assert_eq!(probe.ticks.load(Ordering::SeqCst), 10);

    let info = scheduler.job("gated").unwrap();
    assert_eq!((info.run_count, info.skip_count), (1, 1));
}

#[r2e_core::test]
async fn stats_move_as_they_do_under_the_driver() {
    let probe = Probe::default();
    let app = boot(&probe).await;
    let scheduler = app.scheduler();

    let mut names: Vec<_> = scheduler.jobs().into_iter().map(|job| job.name).collect();
    names.sort();
    assert_eq!(names, ["broken", "count", "gated"]);

    let before = scheduler.job("count").unwrap();
    assert_eq!(before.run_count, 0);
    assert!(before.last_run.is_none() && before.last_duration.is_none());

    scheduler.run("count").await;

    let after = scheduler.job("count").unwrap();
    assert_eq!(after.run_count, 1);
    assert!(after.last_run.is_some() && after.last_duration.is_some());
    // No driver holds a deadline for the job.
    assert!(after.next_run.is_none());
    // The same registry the app's own endpoints read.
    let bean = app.bean::<r2e_scheduler::ScheduledJobRegistry>();
    assert_eq!(bean.job("count").unwrap().run_count, 1);
}

#[r2e_core::test]
#[should_panic(expected = "the job blew up")]
async fn a_panicking_job_fails_the_test_that_ran_it() {
    let app = boot(&Probe::default()).await;
    app.scheduler().run("broken").await;
}

#[r2e_core::test]
async fn a_panicking_job_is_counted() {
    let app = boot(&Probe::default()).await;
    let scheduler = app.scheduler();

    let runner = scheduler.clone();
    let joined = r2e_core::rt::spawn(async move { runner.run("broken").await }).await;

    assert!(joined.unwrap_err().is_panic());
    assert_eq!(scheduler.job("broken").unwrap().panic_count, 1);
}

#[r2e_core::test]
#[should_panic(expected = "no scheduled job named `cuont` — registered jobs: [\"count\", \"gated\", \"broken\"]")]
async fn an_unknown_job_names_the_ones_that_exist() {
    let app = boot(&Probe::default()).await;
    app.scheduler().run("cuont").await;
}

#[r2e_core::test]
#[should_panic(expected = "`scheduler.enabled = false`")]
async fn a_disabled_scheduler_has_no_jobs_to_run() {
    let app = TestApp::boot_plain_env::<JobsApp>(Probe::default(), |b| {
        let mut config = R2eConfig::empty();
        config.set("scheduler.enabled", ConfigValue::Bool(false));
        b.override_config(config)
    })
    .await;
    app.scheduler();
}
