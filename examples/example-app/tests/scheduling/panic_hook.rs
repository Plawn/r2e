//! `AppBuilder::on_panic` end to end for background work (ticket #1027): a
//! real `#[scheduled]` method and a real `#[async_exec]` method, wired through
//! the `Executor`/`Scheduler` plugins exactly as an app does — no hand-built
//! `PoolExecutor::with_panic_hook_slot`. The scheduler cases run under both
//! `scheduler.executor` modes: the dedicated pool must share the app's hook
//! slot like the shared one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::r2e_executor::{Executor, JobHandle, PoolExecutor, RejectedError};
use r2e::r2e_scheduler::Scheduler;
use r2e::type_list::BeanAccess;
use r2e::{PanicOrigin, PanicReport};

/// One hook call: `(message, label, origin)` with the origin flattened to
/// `"scheduled:<task>"` / `"executor:<job>"` / `"http"`.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

fn recording_hook(seen: &Seen) -> impl Fn(&PanicReport<'_>) + Send + Sync + 'static {
    let seen = Arc::clone(seen);
    move |report| {
        let origin = match report.origin() {
            PanicOrigin::Scheduled { task } => format!("scheduled:{task}"),
            PanicOrigin::Executor { job } => format!("executor:{}", job.unwrap_or("<none>")),
            PanicOrigin::Http { .. } => "http".to_owned(),
        };
        seen.lock()
            .unwrap()
            .push((report.message().to_owned(), report.label().to_owned(), origin));
    }
}

// ─── #[scheduled] ───

#[derive(Clone)]
pub struct PanickyTicker;

#[bean]
impl PanickyTicker {
    pub fn new() -> Self {
        Self
    }

    // 1h cadence: only the initial tick fires during the test, so the hook
    // must see exactly one report.
    #[scheduled(every = "1h", name = "panicky_tick")]
    async fn tick(&self) {
        panic!("tick boom");
    }
}

/// Boot the app for real (the scheduler driver only starts at serve time),
/// wait for the hook to observe the tick panic, then abort the boot from a
/// startup hook so `run_with_listener` returns.
async fn scheduled_panic_reaches_the_hook(executor_mode: &str) {
    let seen: Seen = Arc::default();
    let yaml = format!("scheduler:\n  executor: {executor_mode}\n");
    let seen_wait = Arc::clone(&seen);
    let app = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(&yaml).unwrap())
        .load_config::<()>()
        .on_panic(recording_hook(&seen))
        .plugin(Executor)
        .plugin(Scheduler)
        .register::<PanickyTicker>()
        .build_state()
        .await
        .on_start(move |_state| async move {
            let fired = r2e::rt::timeout(Duration::from_secs(10), async {
                while seen_wait.lock().unwrap().is_empty() {
                    r2e::rt::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
            // Let a (wrong) second report land before the assertions.
            r2e::rt::sleep(Duration::from_millis(100)).await;
            let why = if fired.is_ok() { "hook fired" } else { "hook never fired" };
            Err::<(), Box<dyn std::error::Error + Send + Sync>>(why.into())
        });

    let listener = r2e::rt::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = r2e::rt::timeout(
        Duration::from_secs(20),
        app.prepare("127.0.0.1:0").run_with_listener(listener),
    )
    .await
    .expect("run() must return once the startup hook aborts the boot")
    .expect_err("the startup hook aborts the boot");
    assert!(
        err.to_string().contains("hook fired"),
        "the on_panic hook must fire for a panicking #[scheduled] tick: {err}"
    );

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [(
            "tick boom".to_owned(),
            "panicky_tick".to_owned(),
            "scheduled:panicky_tick".to_owned(),
        )],
        "exactly one report, with the Scheduled origin and the task name"
    );
}

#[r2e::test]
async fn a_scheduled_panic_on_the_shared_pool_reaches_on_panic() {
    scheduled_panic_reaches_the_hook("shared").await;
}

#[r2e::test]
async fn a_scheduled_panic_on_the_dedicated_pool_reaches_on_panic() {
    scheduled_panic_reaches_the_hook("dedicated").await;
}

// ─── #[async_exec] ───

#[derive(Clone)]
pub struct PanickyWorker {
    executor: PoolExecutor,
}

#[bean]
impl PanickyWorker {
    pub fn new(executor: PoolExecutor) -> Self {
        Self { executor }
    }

    #[async_exec]
    async fn crunch(&self) -> u32 {
        panic!("job boom");
    }
}

#[r2e::test]
async fn an_async_exec_panic_reaches_on_panic_with_the_method_name() {
    let seen: Seen = Arc::default();
    let app = AppBuilder::new()
        .on_panic(recording_hook(&seen))
        .plugin(Executor)
        .register::<PanickyWorker>()
        .build_state()
        .await;

    let worker = app.state().get::<PanickyWorker>();
    let handle: Result<JobHandle<u32>, RejectedError> = worker.crunch();
    let err = handle
        .expect("submit ok")
        .await
        .expect_err("a panicking job is a failed job");
    assert!(err.is_panic(), "containment unchanged: the job reports a panic");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [(
            "job boom".to_owned(),
            "crunch".to_owned(),
            "executor:crunch".to_owned(),
        )],
        "exactly one report, with the Executor origin and the method name"
    );
}
