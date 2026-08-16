use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_scheduler::{
    ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef, SchedulerHandle,
};
use tokio_util::sync::CancellationToken;

use crate::support::{await_next_run, counting_task, test_pool};

// ── A closed pool is terminal on EVERY submission site ─────────────────────
//
// With no job due for an hour, the trigger-now submission is the only one that
// can observe the closed pool. If it just reported `false`, the driver would
// park indefinitely — holding the bean graph alive as a tracked task.

#[r2e_core::test]
async fn trigger_now_against_a_closed_pool_stops_the_driver() {
    let runs = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "hourly",
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_secs(3600).unwrap(),
            initial_delay: Duration::from_secs(3600),
        },
        runs.clone(),
    );

    let pool = test_pool();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        pool.clone(),
        ScheduledJobRegistry::new(),
        commands,
    ));

    pool.shutdown();
    assert!(
        !handle.trigger_now("hourly").await,
        "a trigger against a closed pool must report false"
    );

    // Note: the token is deliberately NOT cancelled — the driver must stop on
    // its own, the way it does for a cadence submission against a closed pool.
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect(
            "a closed pool must be terminal at the trigger-now submission site too: with no \
             job due, nothing else would ever observe it and the driver would park forever",
        )
        .expect("driver task must not panic");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no tick can have run");
}

#[r2e_core::test]
async fn a_trigger_now_factory_panic_leaves_the_cadence_deadline_alone() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let task = ScheduledTaskDef::new(
        "oob_panic",
        // A plain interval fires immediately; the delay keeps the only cadence
        // fire far outside the test so the OOB tick is the sole factory call.
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_millis(60_000).unwrap(),
            initial_delay: Duration::from_secs(30),
        },
        (),
        move |()| {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("tick factory exploded");
            #[allow(unreachable_code)]
            async {}
        },
    );

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    tokio::time::sleep(Duration::from_millis(100)).await;
    let armed_deadline = await_next_run(&registry, "oob_panic").await;

    assert!(
        !handle.trigger_now("oob_panic").await,
        "a tick whose factory panicked did not run"
    );
    let disabled = registry.job("oob_panic").expect("job info");
    assert!(disabled.paused, "the factory panic must disable the job");
    assert_eq!(disabled.panic_count, 1, "the panic is accounted for");
    assert_eq!(
        disabled.next_run,
        Some(armed_deadline),
        "an out-of-band tick never touches the regular schedule: the cadence \
         deadline (and its telemetry) survives the panic"
    );

    assert!(
        handle.resume("oob_panic").await,
        "resume must report true: the job still holds a deadline"
    );
    assert_eq!(
        await_next_run(&registry, "oob_panic").await,
        armed_deadline,
        "resume keeps an armed deadline instead of re-arming from now"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only the OOB tick was built");

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}
