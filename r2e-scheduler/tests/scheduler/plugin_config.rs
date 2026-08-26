//! Feature B — scheduler plugin config (`scheduler.*`): dedicated pool selection
//! and the standard `scheduler.enabled` gate.
//!
//! These are non-serve tests: `configure` (where the executor is resolved and a
//! dedicated pool is built) runs during `build_state`'s deferred phase, so the
//! dedicated branch is exercised without starting a server.

use r2e_core::config::R2eConfig;
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;
use r2e_core::rt::CancelToken;
use r2e_executor::Executor;
use r2e_scheduler::{ScheduledJobRegistry, Scheduler};

#[r2e_core::test]
async fn shared_executor_is_the_default() {
    // No scheduler config at all → shared pool, unchanged behavior.
    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;
    let _reg: ScheduledJobRegistry = app.state().get::<ScheduledJobRegistry>();
}

#[r2e_core::test]
async fn dedicated_executor_config_builds_a_private_pool() {
    // Exercises `resolve_executor`'s dedicated branch (pool built + drain hook
    // registered) during build_state.
    let config = R2eConfig::from_yaml_str(
        "scheduler:\n  executor: dedicated\n  max-concurrent: 4\n  queue-capacity: 16\n  shutdown-timeout: 1s\n",
    )
    .unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    // Provided beans present regardless of executor mode.
    let _reg: ScheduledJobRegistry = app.state().get::<ScheduledJobRegistry>();
    let _tok: CancelToken = app.state().get::<CancelToken>();
}

#[r2e_core::test]
#[should_panic(expected = "Invalid `scheduler.executor`")]
async fn invalid_executor_value_panics_at_boot() {
    let config = R2eConfig::from_yaml_str("scheduler:\n  executor: bogus\n").unwrap();
    let _ = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;
}

#[r2e_core::test]
async fn disabled_scheduler_boots_and_keeps_beans() {
    // `scheduler.enabled: false` skips the plugin's surface effects (the
    // serve hook that starts tasks) but keeps the provided beans in the graph.
    let config = R2eConfig::from_yaml_str("scheduler:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    let reg: ScheduledJobRegistry = app.state().get::<ScheduledJobRegistry>();
    let _tok: CancelToken = app.state().get::<CancelToken>();
    // No tasks were started (serve hook skipped), so the registry is empty.
    assert!(
        reg.list_jobs().is_empty(),
        "disabled scheduler must not register/start jobs"
    );
}

#[r2e_core::test]
async fn disabled_scheduler_still_collects_tasks() {
    // The disabled scheduler's *promise*: tasks are collected but never
    // started. That only holds while `setup()`'s `TaskRegistryHandle` survives
    // the gate — gating setup actions on `<prefix>.enabled` removed the handle
    // and turned every `#[scheduled]`/`schedule_task` registration into a boot
    // panic ("Scheduler not installed").
    use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
    use r2e_scheduler::{
        extract_tasks, AppBuilderSchedulerExt, ScheduleConfig, ScheduledTaskDef,
    };

    let config = R2eConfig::from_yaml_str("scheduler:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    // The registry handle exists even though the plugin is disabled…
    assert!(
        app.get_plugin_data::<TaskRegistryHandle>().is_some(),
        "TaskRegistryHandle must survive `scheduler.enabled = false`"
    );

    // …so registration works instead of panicking…
    let app = app.schedule_task(ScheduledTaskDef::from_fn(
        "collected_while_disabled",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_secs(60).unwrap()),
        || async {},
    ));

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("registry should exist");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 1, "the task was collected");
    assert_eq!(tasks[0].name(), "collected_while_disabled");

    // …and nothing was started: the serve hook is a build effect, dropped.
    assert!(
        app.state()
            .get::<ScheduledJobRegistry>()
            .list_jobs()
            .is_empty(),
        "collected, never started"
    );
}
