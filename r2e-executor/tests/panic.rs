//! Panic reporting on the pool (ticket #1027): a panicking job reaches the
//! app's `on_panic` hook exactly once with an `Executor`/`Scheduled` origin,
//! emits exactly one `r2e::panic` log line, and — the regression this exists
//! for — the pool's bookkeeping survives the unwind: the permit, the drain
//! count, `completed`, and the idle notification all land, so a graceful
//! shutdown after a panicked job returns promptly instead of waiting out its
//! whole timeout.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::{PanicHook, PanicHookSlot, PanicOrigin, PANIC_TARGET};
use r2e_executor::{ExecutorConfig, PoolExecutor};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

// ── Minimal event capture ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Rec {
    target: String,
    fields: HashMap<String, String>,
}

impl Rec {
    fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
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

#[derive(Default, Clone)]
struct Capture {
    events: Arc<Mutex<Vec<Rec>>>,
}

impl Capture {
    fn panics(&self) -> Vec<Rec> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.target == PANIC_TARGET)
            .cloned()
            .collect()
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldRecorder(&mut fields));
        self.events.lock().unwrap().push(Rec {
            target: event.metadata().target().to_string(),
            fields,
        });
    }
}

// ── Hook plumbing ───────────────────────────────────────────────────────────

/// Owned mirror of [`PanicOrigin`], so a hook invocation can be kept past the
/// report's borrow and asserted on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeenOrigin {
    Scheduled(String),
    Executor(Option<String>),
}

/// `(message, label, origin)` of every hook invocation, in order.
type Reports = Arc<Mutex<Vec<(String, String, SeenOrigin)>>>;

fn recording_hook() -> (Reports, PanicHookSlot) {
    let seen: Reports = Arc::new(Mutex::new(Vec::new()));
    let seen_h = Arc::clone(&seen);
    let hook: PanicHook = Arc::new(move |report| {
        let origin = match report.origin() {
            PanicOrigin::Scheduled { task } => SeenOrigin::Scheduled(task.to_owned()),
            PanicOrigin::Executor { job } => SeenOrigin::Executor(job.map(str::to_owned)),
            other => panic!("unexpected origin on the pool: {other:?}"),
        };
        seen_h.lock().unwrap().push((
            report.message().to_owned(),
            report.label().to_owned(),
            origin,
        ));
    });
    let slot = PanicHookSlot::default();
    slot.set(hook);
    (seen, slot)
}

fn pool_with_hook() -> (
    PoolExecutor,
    Reports,
    Capture,
    tracing::subscriber::DefaultGuard,
) {
    let (seen, slot) = recording_hook();
    let exec = PoolExecutor::with_panic_hook_slot(ExecutorConfig::default(), slot);
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(Registry::default().with(capture.clone()));
    (exec, seen, capture, guard)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_panicking_job_reports_the_executor_origin_once() {
    let (exec, seen, capture, _guard) = pool_with_hook();

    let handle = exec
        .submit(async {
            panic!("job boom");
            #[allow(unreachable_code)]
            ()
        })
        .expect("submit ok");
    let err = handle.await.expect_err("the job must fail");
    assert!(err.is_panic(), "the failure must still read as a panic");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(
            "job boom".to_owned(),
            "<unnamed>".to_owned(),
            SeenOrigin::Executor(None),
        )],
        "the hook fires exactly once, with the Executor origin"
    );

    let events = capture.panics();
    assert_eq!(events.len(), 1, "exactly one r2e::panic line: {events:?}");
    assert_eq!(events[0].field("panic_message"), Some("job boom"));
    assert_eq!(events[0].field("job"), Some("<unnamed>"));
}

#[tokio::test]
async fn a_named_job_reports_its_name() {
    let (exec, seen, capture, _guard) = pool_with_hook();

    let handle = exec
        .submit_named("send_report", async {
            panic!("named boom");
            #[allow(unreachable_code)]
            ()
        })
        .expect("submit ok");
    assert!(handle.await.expect_err("must fail").is_panic());

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(
            "named boom".to_owned(),
            "send_report".to_owned(),
            SeenOrigin::Executor(Some("send_report".to_owned())),
        )]
    );
    let events = capture.panics();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].field("job"), Some("send_report"));
}

#[tokio::test]
async fn a_scheduled_tagged_job_reports_the_scheduled_origin() {
    let (exec, seen, capture, _guard) = pool_with_hook();

    let handle = exec
        .submit_scheduled("nightly-cleanup", async {
            panic!("tick boom");
            #[allow(unreachable_code)]
            ()
        })
        .expect("submit ok");
    assert!(handle.await.expect_err("must fail").is_panic());

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(
            "tick boom".to_owned(),
            "nightly-cleanup".to_owned(),
            SeenOrigin::Scheduled("nightly-cleanup".to_owned()),
        )],
        "a tick submitted through submit_scheduled reports as Scheduled, not Executor"
    );
    let events = capture.panics();
    assert_eq!(events.len(), 1, "the pool is the single reporter");
    assert_eq!(events[0].field("task"), Some("nightly-cleanup"));
    assert_eq!(events[0].field("job"), None, "no executor-shaped field");
}

#[tokio::test]
async fn a_detached_job_reports_too() {
    let (exec, seen, _capture, _guard) = pool_with_hook();

    exec.submit_detached(async {
        panic!("detached boom");
    });

    // No handle to await: poll the hook.
    tokio::time::timeout(Duration::from_secs(5), async {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the hook must fire for a detached job");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[(
            "detached boom".to_owned(),
            "<unnamed>".to_owned(),
            SeenOrigin::Executor(None),
        )]
    );
}

/// A hook is observability: its own panic must not take the pool's worker with
/// it — the job's handle still resolves as a plain panic and a second
/// `r2e::panic` line records the hook's own explosion.
#[tokio::test]
async fn a_panicking_hook_is_contained() {
    let slot = PanicHookSlot::default();
    slot.set(Arc::new(|_| panic!("hook exploded")));
    let exec = PoolExecutor::with_panic_hook_slot(ExecutorConfig::default(), slot);
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(Registry::default().with(capture.clone()));

    let handle = exec
        .submit(async {
            panic!("job boom");
            #[allow(unreachable_code)]
            ()
        })
        .expect("submit ok");
    assert!(handle.await.expect_err("must fail").is_panic());

    let events = capture.panics();
    assert_eq!(events.len(), 2, "job line + hook line: {events:?}");
    assert_eq!(events[0].field("panic_message"), Some("job boom"));
    assert_eq!(events[1].field("panic_message"), Some("hook exploded"));

    // The pool is still healthy.
    let ok = exec.submit(async { 41 + 1 }).expect("submit ok");
    assert_eq!(ok.await.expect("healthy job"), 42);
}

/// The regression the poll-level catch exists for: before #1027 a panicking
/// job unwound past the permit drop, the drain-count decrement, the
/// `completed` increment and the idle notification — so `shutdown_graceful`
/// after a panicked job sat out its whole timeout and `completed`
/// undercounted.
#[tokio::test]
async fn bookkeeping_survives_a_panicking_job() {
    let exec = PoolExecutor::new(ExecutorConfig {
        max_concurrent: 2,
        queue_capacity: 8,
        shutdown_timeout: Duration::from_secs(5),
    });

    let boom = exec
        .submit(async {
            panic!("boom");
            #[allow(unreachable_code)]
            ()
        })
        .expect("submit ok");
    assert!(boom.await.expect_err("must fail").is_panic());

    let m = exec.metrics();
    assert_eq!(m.completed, 1, "a panicked job still counts as completed");
    assert_eq!(m.running, 0, "its permit must have been released");

    // Would previously block until the 2s deadline because the leaked drain
    // count never reached zero.
    let start = std::time::Instant::now();
    let drained = exec.shutdown_graceful(Duration::from_secs(2)).await;
    assert!(drained, "an idle pool must drain immediately");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "drain must be prompt, not a timeout expiry: {:?}",
        start.elapsed()
    );
}
