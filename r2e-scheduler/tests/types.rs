use r2e_scheduler::{ScheduleConfig, ScheduledResult, ScheduledTask, ScheduledTaskDef, extract_tasks};
use std::any::Any;
use std::time::Duration;

fn noop_task_def(name: &str, schedule: ScheduleConfig) -> ScheduledTaskDef<()> {
    ScheduledTaskDef {
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
