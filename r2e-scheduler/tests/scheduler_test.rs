use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    extract_tasks, start_jobs, ScheduleConfig, ScheduledJobRegistry, ScheduledTask,
    ScheduledTaskDef, SchedulerCommands,
};
use tokio_util::sync::CancellationToken;

// ── Helpers ────────────────────────────────────────────────────────────────

/// A default-configured pool. Scheduled tick bodies are submitted here; the
/// pool is kept alive by the spawned schedule task (`start` moves it in).
fn test_pool() -> PoolExecutor {
    PoolExecutor::new(ExecutorConfig::default())
}

fn counting_task(
    name: &str,
    schedule: ScheduleConfig,
    counter: Arc<AtomicUsize>,
) -> ScheduledTaskDef<Arc<AtomicUsize>> {
    ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: name.to_string(),
        schedule,
        state: counter,
        task: Box::new(|c| {
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        }),
    }
}

/// Convert boxed tasks into jobs and hand them to a single driver — the
/// production path ([`start_jobs`]) exercised directly from tests.
fn start_jobs_helper(
    tasks: impl IntoIterator<Item = Box<dyn ScheduledTask>>,
    token: CancellationToken,
    pool: PoolExecutor,
) {
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        token,
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );
}

fn start_task(task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>) -> CancellationToken {
    let token = CancellationToken::new();
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    start_jobs_helper([boxed], token.clone(), test_pool());
    token
}

fn boxed_task(
    task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>,
) -> Box<dyn std::any::Any + Send> {
    let trait_obj: Box<dyn ScheduledTask> = Box::new(task);
    Box::new(trait_obj)
}

/// With `start_paused = true`, `sleep()` cooperatively yields to spawned tasks
/// while auto-advancing the mock clock. This is more reliable than
/// `advance() + yield_now()` which only gives one poll per yield.
async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

// ── Phase 3: Interval tests (start_paused = true) ─────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_task_runs_repeatedly() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "repeat",
        ScheduleConfig::Interval(Duration::from_millis(100)),
        counter.clone(),
    );
    let _token = start_task(task);

    sleep_ms(350).await;

    let count = counter.load(Ordering::SeqCst);
    // First tick is immediate, then at 100ms, 200ms, 300ms = 4 ticks
    assert!(count >= 3, "expected >= 3 executions, got {count}");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_task_stops_on_cancel() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "cancel_me",
        ScheduleConfig::Interval(Duration::from_millis(100)),
        counter.clone(),
    );
    let token = start_task(task);

    sleep_ms(250).await;
    let count_before = counter.load(Ordering::SeqCst);
    assert!(count_before >= 2, "expected >= 2 before cancel, got {count_before}");

    token.cancel();
    // Give the task a chance to observe cancellation
    tokio::task::yield_now().await;

    let count_snapshot = counter.load(Ordering::SeqCst);
    sleep_ms(200).await;
    let count_after = counter.load(Ordering::SeqCst);
    assert_eq!(
        count_snapshot, count_after,
        "counter should not increment after cancel"
    );
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_with_initial_delay() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "delayed",
        ScheduleConfig::IntervalWithDelay {
            interval: Duration::from_millis(100),
            initial_delay: Duration::from_millis(200),
        },
        counter.clone(),
    );
    let _token = start_task(task);

    // Before delay expires
    sleep_ms(150).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0, "should not run during delay");

    // After delay + first interval tick (200ms delay + immediate first tick)
    sleep_ms(100).await;
    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 1, "expected >= 1 after delay, got {count}");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_cancel_during_delay() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "cancel_in_delay",
        ScheduleConfig::IntervalWithDelay {
            interval: Duration::from_millis(100),
            initial_delay: Duration::from_millis(500),
        },
        counter.clone(),
    );
    let token = start_task(task);

    sleep_ms(100).await;
    token.cancel();
    tokio::task::yield_now().await;

    sleep_ms(600).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0, "should never have run");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_task_state_accessible() {
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "logger".to_string(),
        schedule: ScheduleConfig::Interval(Duration::from_millis(100)),
        state: log.clone(),
        task: Box::new(|log: Arc<Mutex<Vec<String>>>| {
            Box::pin(async move {
                log.lock().unwrap().push("tick".to_string());
            })
                as Pin<Box<dyn Future<Output = ()> + Send>>
        }),
    };
    let _token = start_task(task);

    sleep_ms(350).await;

    let entries = log.lock().unwrap();
    assert!(entries.len() >= 3, "expected >= 3 log entries, got {}", entries.len());
    assert!(entries.iter().all(|e| e == "tick"));
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn interval_task_panic_isolation() {
    let counter = Arc::new(AtomicUsize::new(0));

    // Panicking task — each tick body is submitted to the shared pool, so the
    // panic is contained in the pool job. The schedule loop logs and keeps
    // ticking (it does NOT die on the first panic), and other tasks are
    // unaffected.
    let attempts = Arc::new(AtomicUsize::new(0));
    let a = attempts.clone();
    let panic_task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "panicker".to_string(),
        schedule: ScheduleConfig::Interval(Duration::from_millis(100)),
        state: a,
        task: Box::new(|a: Arc<AtomicUsize>| {
            Box::pin(async move {
                a.fetch_add(1, Ordering::SeqCst);
                panic!("intentional panic");
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }),
    };

    let good_task = counting_task(
        "good",
        ScheduleConfig::Interval(Duration::from_millis(100)),
        counter.clone(),
    );

    let token = CancellationToken::new();
    let pool = test_pool();
    let boxed_panic: Box<dyn ScheduledTask> = Box::new(panic_task);
    let boxed_good: Box<dyn ScheduledTask> = Box::new(good_task);
    start_jobs_helper([boxed_panic, boxed_good], token.clone(), pool);

    sleep_ms(350).await;

    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 3, "good task should be unaffected, got {count}");
    // The panicking schedule loop survived the panic and kept re-ticking.
    let tries = attempts.load(Ordering::SeqCst);
    assert!(
        tries >= 3,
        "panicking task loop should keep ticking after a contained panic, got {tries}"
    );
}

// ── Phase 4: Cron tests (real time, multi-thread) ──────────────────────────

#[r2e_core::test]
async fn cron_task_runs() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "cron_every_sec",
        ScheduleConfig::Cron("* * * * * *".to_string()),
        counter.clone(),
    );
    let _token = start_task(task);

    tokio::time::sleep(Duration::from_millis(2500)).await;

    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 1, "cron should have run at least once, got {count}");
}

#[r2e_core::test]
async fn cron_invalid_expression_no_panic() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "bad_cron",
        ScheduleConfig::Cron("not a valid cron".to_string()),
        counter.clone(),
    );
    let _token = start_task(task);

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0, "invalid cron should never fire");
}

#[r2e_core::test]
async fn cron_task_stops_on_cancel() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "cron_cancel",
        ScheduleConfig::Cron("* * * * * *".to_string()),
        counter.clone(),
    );
    let token = start_task(task);

    tokio::time::sleep(Duration::from_millis(1500)).await;
    token.cancel();

    // Wait for cancellation to settle
    tokio::time::sleep(Duration::from_millis(100)).await;
    let count_snapshot = counter.load(Ordering::SeqCst);

    tokio::time::sleep(Duration::from_millis(2000)).await;
    let count_after = counter.load(Ordering::SeqCst);

    assert_eq!(count_snapshot, count_after, "counter should not increment after cancel");
}

#[r2e_core::test]
async fn cron_multiple_executions() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "cron_multi",
        ScheduleConfig::Cron("* * * * * *".to_string()),
        counter.clone(),
    );
    let _token = start_task(task);

    tokio::time::sleep(Duration::from_millis(3500)).await;

    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 2, "cron should have run >= 2 times, got {count}");
}

// ── Phase 5: Lifecycle tests ───────────────────────────────────────────────

#[test]
fn extract_tasks_from_boxed() {
    let task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "boxed".to_string(),
        schedule: ScheduleConfig::Interval(Duration::from_secs(1)),
        state: (),
        task: Box::new(|_| Box::pin(async {})),
    };
    let b = boxed_task(task);
    let tasks = extract_tasks(vec![b]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name(), "boxed");
}

#[test]
fn extract_tasks_empty_vec() {
    let tasks = extract_tasks(vec![]);
    assert!(tasks.is_empty());
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn multiple_tasks_all_start() {
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));
    let c3 = Arc::new(AtomicUsize::new(0));

    let token = CancellationToken::new();
    let pool = test_pool();
    let tasks: Vec<Box<dyn ScheduledTask>> =
        [("t1", c1.clone()), ("t2", c2.clone()), ("t3", c3.clone())]
            .into_iter()
            .map(|(name, counter)| {
                Box::new(counting_task(
                    name,
                    ScheduleConfig::Interval(Duration::from_millis(100)),
                    counter,
                )) as Box<dyn ScheduledTask>
            })
            .collect();
    start_jobs_helper(tasks, token.clone(), pool);

    sleep_ms(350).await;

    for (name, c) in [("t1", &c1), ("t2", &c2), ("t3", &c3)] {
        let count = c.load(Ordering::SeqCst);
        assert!(count >= 3, "{name} should have run >= 3 times, got {count}");
    }
}

#[test]
fn task_name_via_trait() {
    let task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "trait_name".to_string(),
        schedule: ScheduleConfig::Interval(Duration::from_secs(1)),
        state: (),
        task: Box::new(|_| Box::pin(async {})),
    };
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    assert_eq!(boxed.name(), "trait_name");
}

#[test]
fn task_schedule_via_trait() {
    let task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "trait_schedule".to_string(),
        schedule: ScheduleConfig::Cron("0 0 * * * *".to_string()),
        state: (),
        task: Box::new(|_| Box::pin(async {})),
    };
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    match boxed.schedule() {
        ScheduleConfig::Cron(expr) => assert_eq!(expr, "0 0 * * * *"),
        _ => panic!("expected Cron schedule"),
    }
}

// ── Phase 6: State tests (start_paused = true) ────────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn state_cloned_per_execution() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "clone_counter",
        ScheduleConfig::Interval(Duration::from_millis(100)),
        counter.clone(),
    );
    let _token = start_task(task);

    sleep_ms(550).await;

    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 5, "expected >= 5 increments, got {count}");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn concurrent_tasks_shared_state() {
    let shared = Arc::new(AtomicUsize::new(0));

    let token = CancellationToken::new();
    let pool = test_pool();
    let tasks: Vec<Box<dyn ScheduledTask>> = ["a", "b"]
        .into_iter()
        .map(|name| {
            Box::new(counting_task(
                name,
                ScheduleConfig::Interval(Duration::from_millis(100)),
                shared.clone(),
            )) as Box<dyn ScheduledTask>
        })
        .collect();
    start_jobs_helper(tasks, token.clone(), pool);

    sleep_ms(350).await;

    let total = shared.load(Ordering::SeqCst);
    // Each task runs >= 3 times (immediate + 100ms + 200ms + 300ms), so total >= 6
    assert!(total >= 6, "expected total >= 6, got {total}");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn concurrent_tasks_independent_state() {
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));

    let token = CancellationToken::new();
    let task1 = counting_task("ind1", ScheduleConfig::Interval(Duration::from_millis(100)), c1.clone());
    let task2 = counting_task("ind2", ScheduleConfig::Interval(Duration::from_millis(200)), c2.clone());

    let pool = test_pool();
    let b1: Box<dyn ScheduledTask> = Box::new(task1);
    let b2: Box<dyn ScheduledTask> = Box::new(task2);
    start_jobs_helper([b1, b2], token.clone(), pool);

    sleep_ms(450).await;

    let v1 = c1.load(Ordering::SeqCst);
    let v2 = c2.load(Ordering::SeqCst);
    assert!(v1 >= 4, "fast task should have run >= 4 times, got {v1}");
    assert!(v2 >= 2, "slow task should have run >= 2 times, got {v2}");
    assert!(v1 > v2, "fast task ({v1}) should exceed slow task ({v2})");
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn state_mutations_visible_via_arc_mutex() {
    let log: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));

    let task = ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        name: "mutator".to_string(),
        schedule: ScheduleConfig::Interval(Duration::from_millis(100)),
        state: log.clone(),
        task: Box::new(|log: Arc<Mutex<Vec<i32>>>| {
            Box::pin(async move {
                let mut v = log.lock().unwrap();
                let next = v.len() as i32 + 1;
                v.push(next);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }),
    };
    let _token = start_task(task);

    sleep_ms(350).await;

    let entries = log.lock().unwrap();
    assert!(entries.len() >= 3, "expected >= 3 entries, got {}", entries.len());
    // Verify sequential mutations are visible
    for (i, val) in entries.iter().enumerate() {
        assert_eq!(*val, (i + 1) as i32, "entry {i} should be {}", i + 1);
    }
}

// ── Driver properties: cross-job independence + per-job non-overlap ─────────
//
// These run on real time (multi-thread) because they observe genuine
// concurrency between and within jobs — the whole point of the single-driver
// model. The driver must never await one job's tick inline (that would
// serialize other jobs), yet must never fire the SAME job again until its
// previous tick completes.

#[r2e_core::test]
async fn jobs_run_concurrently_not_serialized() {
    let slow_ticks = Arc::new(AtomicUsize::new(0));
    let fast_ticks = Arc::new(AtomicUsize::new(0));

    // Job A: 50ms cadence, each tick sleeps ~300ms (long-running).
    let sa = slow_ticks.clone();
    let slow = ScheduledTaskDef::new(
        "slow",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        sa,
        |sa: Arc<AtomicUsize>| async move {
            sa.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
        },
    );

    // Job B: 50ms cadence, fast counter.
    let fast = counting_task(
        "fast",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        fast_ticks.clone(),
    );

    let token = CancellationToken::new();
    let sb: Box<dyn ScheduledTask> = Box::new(slow);
    let fb: Box<dyn ScheduledTask> = Box::new(fast);
    start_jobs_helper([sb, fb], token.clone(), test_pool());

    tokio::time::sleep(Duration::from_millis(400)).await;
    token.cancel();

    // While A's first tick is still sleeping, B must keep ticking many times.
    let f = fast_ticks.load(Ordering::SeqCst);
    assert!(
        f >= 4,
        "fast job must keep ticking while the slow job runs, got {f}"
    );
    assert!(slow_ticks.load(Ordering::SeqCst) >= 1, "slow job should have fired");
}

#[r2e_core::test]
async fn per_job_ticks_never_overlap() {
    // Gauge of concurrent executions of THIS job; `saw_overlap` latches if it
    // ever exceeds 1.
    let concurrent = Arc::new(AtomicUsize::new(0));
    let saw_overlap = Arc::new(AtomicBool::new(false));

    let c = concurrent.clone();
    let o = saw_overlap.clone();
    // 50ms cadence but each tick holds for 150ms: without the non-overlap
    // invariant, ticks would pile up and the gauge would exceed 1.
    let task = ScheduledTaskDef::new(
        "overlap_guard",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        (c, o),
        |(c, o): (Arc<AtomicUsize>, Arc<AtomicBool>)| async move {
            let live = c.fetch_add(1, Ordering::SeqCst) + 1;
            if live > 1 {
                o.store(true, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            c.fetch_sub(1, Ordering::SeqCst);
        },
    );

    let token = CancellationToken::new();
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    start_jobs_helper([boxed], token.clone(), test_pool());

    tokio::time::sleep(Duration::from_millis(500)).await;
    token.cancel();

    assert!(
        !saw_overlap.load(Ordering::SeqCst),
        "a job's ticks must never overlap with themselves"
    );
}
