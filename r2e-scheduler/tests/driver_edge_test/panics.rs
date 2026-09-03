use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::rt::CancelToken;
use r2e_core::{PanicHook, PanicHookSlot, PanicOrigin, PANIC_TARGET};
use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerCommands,
};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use crate::support::{counting_task, start_one, test_pool};

// ── A panicking tick is contained and counted ───────────────────────────────

#[r2e_core::test]
async fn panicking_tick_increments_panic_count() {
    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();

    // Struct-literal form; the diverging (`panic!`) body ends in an explicit
    // `()` so never-type inference keeps `Output = ()`.
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
                #[allow(unreachable_code)]
                ()
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

// ── A panicking tick reaches the app's on_panic hook (ticket #1027) ─────────
//
// The pool is the single reporter: the tick submitted through
// `submit_scheduled` reports as `PanicOrigin::Scheduled { task }`, the driver
// logs nothing of its own, and the registry's `panic_count` still moves.

/// Event-only capture of the `r2e::panic` target.
#[derive(Default, Clone)]
struct PanicCapture {
    events: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl PanicCapture {
    fn events(&self) -> Vec<HashMap<String, String>> {
        self.events.lock().unwrap().clone()
    }
}

struct FieldRecorder<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S: Subscriber> Layer<S> for PanicCapture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != PANIC_TARGET {
            return;
        }
        let mut fields = HashMap::new();
        event.record(&mut FieldRecorder(&mut fields));
        self.events.lock().unwrap().push(fields);
    }
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_panicking_tick_reports_the_scheduled_origin_exactly_once() {
    // `(message, label, task)` of every hook call; `task` is `Some` only when
    // the origin really was `Scheduled`.
    type Seen = Arc<Mutex<Vec<(String, String, Option<String>)>>>;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_h = Arc::clone(&seen);
    let hook: PanicHook = Arc::new(move |report| {
        let task = match report.origin() {
            PanicOrigin::Scheduled { task } => Some(task.to_owned()),
            _ => None,
        };
        seen_h
            .lock()
            .unwrap()
            .push((report.message().to_owned(), report.label().to_owned(), task));
    });
    let slot = PanicHookSlot::default();
    slot.set(hook);
    let pool = PoolExecutor::with_panic_hook_slot(ExecutorConfig::default(), slot);

    let capture = PanicCapture::default();
    let _guard = tracing::subscriber::set_default(Registry::default().with(capture.clone()));

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let task = ScheduledTaskDef::new(
        "panicker",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        (),
        |()| async {
            panic!("tick boom");
            #[allow(unreachable_code)]
            ()
        },
    );
    start_one(task, cancel.clone(), pool, registry.clone());

    // Wait for at least one contained panic, then stop the cadence.
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the hook must fire for a panicking tick");
    cancel.cancel();
    // Let the in-flight bookkeeping settle before counting.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let seen = seen.lock().unwrap().clone();
    for (message, label, task) in &seen {
        assert_eq!(message, "tick boom");
        assert_eq!(label, "panicker", "label() is the task name");
        assert_eq!(
            task.as_deref(),
            Some("panicker"),
            "the origin must be Scheduled with the task name"
        );
    }

    // Single reporter: one `r2e::panic` line per hook call — the driver adds
    // no duplicate line of its own — and each carries `task`, not `job`.
    let events = capture.events();
    assert_eq!(
        events.len(),
        seen.len(),
        "one log line per panic: {events:?}"
    );
    for event in &events {
        assert_eq!(event.get("task").map(String::as_str), Some("panicker"));
        assert_eq!(event.get("job"), None, "not reported as an executor job");
        assert_eq!(
            event.get("panic_message").map(String::as_str),
            Some("tick boom")
        );
    }

    // Containment unchanged: the registry still counts the panics.
    let info = registry.job("panicker").expect("job registered");
    assert!(
        info.panic_count as usize >= seen.len().min(1),
        "panic_count must move: {info:?}"
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
    let cancel = CancelToken::new();
    let jobs: Vec<_> = [
        Box::new(panicky) as Box<dyn ScheduledTask>,
        Box::new(healthy) as Box<dyn ScheduledTask>,
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
