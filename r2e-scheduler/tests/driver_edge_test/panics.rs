use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_scheduler::{
    OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerCommands,
};
use tokio_util::sync::CancellationToken;

use crate::support::{counting_task, start_one, test_pool};

// ── A panicking tick is contained and counted ───────────────────────────────

#[r2e_core::test]
async fn panicking_tick_increments_panic_count() {
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();

    // Struct-literal form with the future cast to `Output = ()` so the
    // diverging (`panic!`) body doesn't trip never-type inference.
    let task = ScheduledTaskDef {
        overlap: OverlapPolicy::Skip,
        skip: None,
        name: "panicker".to_string(),
        schedule: ScheduleConfig::Interval(
            r2e_scheduler::PositiveDuration::from_millis(50).unwrap(),
        ),
        state: (),
        task: Box::new(|()| {
            Box::pin(async move {
                panic!("intentional panic in tick");
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }),
    };
    start_one(task, cancel.clone(), test_pool(), registry.clone());

    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    let info = registry.job("panicker").expect("job registered");
    assert!(
        info.panic_count >= 1,
        "at least one contained panic should be recorded, got {}",
        info.panic_count
    );
}

// ── A tick FACTORY that panics ──────────────────────────────────────────────
//
// Tick bodies panic inside the pool, which contains them. Tick *construction*
// runs user code (`state.clone()`, the closure body up to its first await) on
// the driver's own stack: an unguarded panic there unwinds the driver past its
// drain, detaching the ticks already in flight and dropping the tracked future
// that keeps the bean graph alive. The panic is caught at the construction
// site, the broken job is disabled, and the driver keeps driving.

#[r2e_core::test]
async fn a_panicking_tick_factory_disables_its_job_without_killing_the_driver() {
    #[derive(Clone)]
    struct State {
        calls: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
        finished: Arc<std::sync::atomic::AtomicBool>,
    }

    let state = State {
        calls: Arc::new(AtomicUsize::new(0)),
        started: Arc::new(tokio::sync::Notify::new()),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let healthy_runs = Arc::new(AtomicUsize::new(0));

    let panicky = ScheduledTaskDef::new(
        "panicky_factory",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        state.clone(),
        |s: State| {
            let nth = s.calls.fetch_add(1, Ordering::SeqCst);
            // Second construction blows up — synchronously, before any future
            // exists, while the first tick is still running.
            assert!(nth != 1, "tick factory exploded");
            async move {
                if nth == 0 {
                    s.started.notify_one();
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    s.finished.store(true, Ordering::SeqCst);
                }
            }
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let healthy = counting_task(
        "healthy_neighbour",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        healthy_runs.clone(),
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let jobs: Vec<_> = [
        Box::new(panicky) as Box<dyn ScheduledTask>,
        Box::new(healthy) as Box<dyn ScheduledTask>,
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        SchedulerCommands::disconnected(),
    ));

    state.started.notified().await;
    // Long enough for several more fires: the panicking one and a handful of
    // healthy ones.
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(
        healthy_runs.load(Ordering::SeqCst) >= 2,
        "the neighbouring job must keep firing after another job's factory panicked"
    );
    let info = registry.job("panicky_factory").expect("job info");
    assert!(
        info.paused && info.panic_count >= 1,
        "a factory panic must disable the job and be recorded: {info:?}"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("the driver must return after cancellation")
        .expect("the driver task must survive a panic in a tick factory");

    assert!(
        state.finished.load(Ordering::SeqCst),
        "the tick already in flight must still be drained"
    );
}
