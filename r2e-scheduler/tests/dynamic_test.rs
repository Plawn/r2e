//! Dynamic (config-driven) task registration via `AppBuilderSchedulerExt`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::AppBuilder;
use r2e_executor::{Executor, ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    extract_tasks, start_jobs, AppBuilderSchedulerExt, ScheduleConfig, ScheduledJobRegistry,
    ScheduledTaskDef, Scheduler, SchedulerCommands,
};
use tokio_util::sync::CancellationToken;

#[r2e_core::test]
async fn schedule_task_lands_under_scheduler_marker() {
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await
        .schedule_task(ScheduledTaskDef::from_fn(
            "dynamic_one",
            ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_secs(60).unwrap()),
            || async {},
        ));

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "dynamic_one");
    match tasks[0].schedule() {
        ScheduleConfig::Interval(d) => assert_eq!(d.get(), Duration::from_secs(60)),
        _ => panic!("expected Interval"),
    }
}

#[r2e_core::test]
async fn schedule_tasks_registers_a_config_driven_batch() {
    // Simulates patina's case: one task per config entry, same state type.
    let sources = ["alpha", "beta", "gamma"];
    let counter = Arc::new(AtomicUsize::new(0));

    let defs: Vec<_> = sources
        .iter()
        .map(|name| {
            ScheduledTaskDef::new(
                format!("sync_{name}"),
                "5m".parse().expect("valid schedule"),
                counter.clone(),
                |c| async move {
                    c.fetch_add(1, Ordering::SeqCst);
                },
            )
        })
        .collect();

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await
        .schedule_tasks(defs);

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    let mut names: Vec<_> = tasks.iter().map(|t| t.name().to_string()).collect();
    names.sort();
    assert_eq!(names, ["sync_alpha", "sync_beta", "sync_gamma"]);
}

#[r2e_core::test]
async fn dynamic_task_runs_and_stops_on_cancel() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await
        .schedule_task(ScheduledTaskDef::from_fn(
            "ticker",
            ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);

    let token = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        token.clone(),
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        counter.load(Ordering::SeqCst) >= 1,
        "task should have run at least once"
    );

    token.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let snapshot = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        snapshot,
        counter.load(Ordering::SeqCst),
        "counter should not increment after cancel"
    );
}

#[r2e_core::test]
async fn result_returning_closure_logs_instead_of_panicking() {
    let ran = Arc::new(AtomicUsize::new(0));
    let r = ran.clone();

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await
        .schedule_task(ScheduledTaskDef::from_fn(
            "failing",
            ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
            move || {
                let r = r.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>("boom".to_string())
                }
            },
        ));

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());

    let token = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        token.clone(),
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    token.cancel();
    assert!(
        ran.load(Ordering::SeqCst) >= 1,
        "failing task should keep running (errors are logged, not fatal)"
    );
}

#[r2e_core::test]
async fn schedule_task_with_pulls_state_from_bean_context() {
    let counter = Arc::new(AtomicUsize::new(0));

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .provide(counter.clone())
        .build_state()
        .await
        .schedule_task_with(|ctx| {
            ScheduledTaskDef::new(
                "bean_backed",
                ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
                ctx.get::<Arc<AtomicUsize>>(),
                |c| async move {
                    c.fetch_add(1, Ordering::SeqCst);
                },
            )
        });

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "bean_backed");

    // The captured state is the provided bean: ticks land on `counter`.
    let token = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        token.clone(),
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    token.cancel();
    assert!(
        counter.load(Ordering::SeqCst) >= 1,
        "task should tick on the bean pulled from the context"
    );
}

#[r2e_core::test]
async fn schedule_tasks_with_builds_a_batch_from_the_context() {
    let counter = Arc::new(AtomicUsize::new(0));

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .provide(counter.clone())
        .build_state()
        .await
        .schedule_tasks_with(|ctx| {
            let c = ctx.get::<Arc<AtomicUsize>>();
            ["alpha", "beta"]
                .iter()
                .map(|name| {
                    ScheduledTaskDef::new(
                        format!("sync_{name}"),
                        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_secs(60).unwrap()),
                        c.clone(),
                        |c| async move {
                            c.fetch_add(1, Ordering::SeqCst);
                        },
                    )
                })
                .collect::<Vec<_>>()
        });

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    let mut names: Vec<_> = tasks.iter().map(|t| t.name().to_string()).collect();
    names.sort();
    assert_eq!(names, ["sync_alpha", "sync_beta"]);
}

#[r2e_core::test]
#[should_panic(expected = "Scheduler not installed")]
async fn schedule_task_without_plugin_panics() {
    let _ = AppBuilder::new()
        .build_state()
        .await
        .schedule_task(ScheduledTaskDef::from_fn(
            "orphan",
            ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_secs(60).unwrap()),
            || async {},
        ));
}
