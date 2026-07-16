use r2e_scheduler::{ScheduleConfig, ScheduledResult, ScheduledTask, ScheduledTaskDef, extract_tasks};
use std::any::Any;
use std::time::Duration;

fn noop_task_def(name: &str, schedule: ScheduleConfig) -> ScheduledTaskDef<()> {
    ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: name.to_string(),
        schedule,
        state: (),
        task: Box::new(|_| Box::pin(async {})),
    }
}

/// Double-box a `ScheduledTaskDef` so it can round-trip through `extract_tasks`.
fn boxed_task<T: Clone + Send + Sync + 'static>(
    task: ScheduledTaskDef<T>,
) -> Box<dyn Any + Send> {
    let trait_obj: Box<dyn ScheduledTask> = Box::new(task);
    Box::new(trait_obj)
}

// -- ScheduleConfig construction --

#[test]
fn schedule_config_interval() {
    let cfg = ScheduleConfig::Interval(Duration::from_secs(60));
    match cfg {
        ScheduleConfig::Interval(d) => assert_eq!(d, Duration::from_secs(60)),
        _ => panic!("expected Interval"),
    }
}

#[test]
fn schedule_config_interval_with_delay() {
    let cfg = ScheduleConfig::IntervalWithDelay {
        interval: Duration::from_secs(30),
        initial_delay: Duration::from_secs(5),
    };
    match cfg {
        ScheduleConfig::IntervalWithDelay {
            interval,
            initial_delay,
        } => {
            assert_eq!(interval, Duration::from_secs(30));
            assert_eq!(initial_delay, Duration::from_secs(5));
        }
        _ => panic!("expected IntervalWithDelay"),
    }
}

#[test]
fn schedule_config_cron() {
    let expr = "0 */5 * * * *".to_string();
    let cfg = ScheduleConfig::Cron(expr.clone());
    match cfg {
        ScheduleConfig::Cron(e) => assert_eq!(e, expr),
        _ => panic!("expected Cron"),
    }
}

// -- ScheduledResult --

#[test]
fn scheduled_result_unit_noop() {
    ().log_if_err("test_task");
}

#[test]
fn scheduled_result_ok_noop() {
    Ok::<(), String>(()).log_if_err("test_task");
}

#[test]
fn scheduled_result_err_no_panic() {
    Err::<(), _>("fail".to_string()).log_if_err("test_task");
}

// -- extract_tasks --

#[test]
fn extract_tasks_empty() {
    let result = extract_tasks(vec![]);
    assert!(result.is_empty());
}

#[test]
fn extract_tasks_single() {
    let task = noop_task_def("single", ScheduleConfig::Interval(Duration::from_secs(1)));
    let boxed = boxed_task(task);
    let tasks = extract_tasks(vec![boxed]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "single");
}

#[test]
fn extract_tasks_wrong_type_ignored() {
    let wrong: Box<dyn Any + Send> = Box::new(42i32);
    let tasks = extract_tasks(vec![wrong]);
    assert!(tasks.is_empty());
}

#[test]
fn extract_tasks_mixed() {
    let valid = noop_task_def("valid", ScheduleConfig::Interval(Duration::from_secs(1)));
    let valid_boxed = boxed_task(valid);
    let invalid: Box<dyn Any + Send> = Box::new("not a task".to_string());
    let tasks = extract_tasks(vec![valid_boxed, invalid]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "valid");
}

// -- ScheduledTaskDef name/schedule --

#[test]
fn task_def_name_and_schedule() {
    let task = noop_task_def("my_task", ScheduleConfig::Interval(Duration::from_secs(10)));
    assert_eq!(task.name(), "my_task");
    match task.schedule() {
        ScheduleConfig::Interval(d) => assert_eq!(*d, Duration::from_secs(10)),
        _ => panic!("expected Interval"),
    }
}

// -- ScheduledTaskDef constructors --

#[test]
fn task_def_new_captures_state() {
    let task = ScheduledTaskDef::new(
        "with_state",
        ScheduleConfig::Interval(Duration::from_secs(1)),
        42u32,
        |_n| async {},
    );
    assert_eq!(task.name(), "with_state");
    assert_eq!(task.state, 42);
}

#[test]
fn task_def_from_fn_is_stateless() {
    let task = ScheduledTaskDef::from_fn(
        "stateless",
        ScheduleConfig::Interval(Duration::from_secs(1)),
        || async {},
    );
    assert_eq!(task.name(), "stateless");
}

#[test]
fn task_def_new_accepts_result_closures() {
    // Compiles: the closure returns Result<(), E>, wrapped via ScheduledResult.
    let _task = ScheduledTaskDef::new(
        "fallible",
        ScheduleConfig::Interval(Duration::from_secs(1)),
        (),
        |_| async { Err::<(), _>("boom".to_string()) },
    );
}

#[test]
fn into_boxed_any_roundtrips_through_extract_tasks() {
    let task = ScheduledTaskDef::from_fn(
        "roundtrip",
        ScheduleConfig::Interval(Duration::from_secs(1)),
        || async {},
    );
    let tasks = extract_tasks(vec![task.into_boxed_any()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "roundtrip");
}

// -- ScheduleConfig FromStr --

#[test]
fn from_str_duration_is_interval() {
    let cfg: ScheduleConfig = "30s".parse().unwrap();
    match cfg {
        ScheduleConfig::Interval(d) => assert_eq!(d, Duration::from_secs(30)),
        _ => panic!("expected Interval"),
    }

    let cfg: ScheduleConfig = "1h30m".parse().unwrap();
    match cfg {
        ScheduleConfig::Interval(d) => assert_eq!(d, Duration::from_secs(5400)),
        _ => panic!("expected Interval"),
    }
}

#[test]
fn from_str_cron_is_validated() {
    let cfg: ScheduleConfig = "0 */5 * * * *".parse().unwrap();
    match cfg {
        ScheduleConfig::Cron(e) => assert_eq!(e, "0 */5 * * * *"),
        _ => panic!("expected Cron"),
    }
}

#[test]
fn from_str_at_shortcut_is_cron() {
    let cfg: ScheduleConfig = "@hourly".parse().unwrap();
    assert!(matches!(cfg, ScheduleConfig::Cron(_)));
}

#[test]
fn from_str_rejects_garbage() {
    assert!("".parse::<ScheduleConfig>().is_err());
    assert!("abc".parse::<ScheduleConfig>().is_err());
    assert!("0s".parse::<ScheduleConfig>().is_err());
    // whitespace → treated as cron, and it's not a valid expression
    assert!("not a cron".parse::<ScheduleConfig>().is_err());
}

// -- ScheduleConfig FromConfigValue --

#[test]
fn from_config_value_string_and_integer() {
    use r2e_core::config::{ConfigValue, FromConfigValue};

    let cfg =
        ScheduleConfig::from_config_value(&ConfigValue::String("5m".to_string()), "app.sched")
            .unwrap();
    assert!(matches!(cfg, ScheduleConfig::Interval(d) if d == Duration::from_secs(300)));

    // Integer = seconds, mirroring #[scheduled(every = 30)]
    let cfg = ScheduleConfig::from_config_value(&ConfigValue::Integer(30), "app.sched").unwrap();
    assert!(matches!(cfg, ScheduleConfig::Interval(d) if d == Duration::from_secs(30)));
}

#[test]
fn from_config_value_rejects_bad_values() {
    use r2e_core::config::{ConfigValue, FromConfigValue};

    assert!(
        ScheduleConfig::from_config_value(&ConfigValue::Integer(0), "app.sched").is_err(),
        "zero seconds rejected"
    );
    assert!(ScheduleConfig::from_config_value(&ConfigValue::Bool(true), "app.sched").is_err());
    assert!(ScheduleConfig::from_config_value(
        &ConfigValue::String("nope".to_string()),
        "app.sched"
    )
    .is_err());
}
