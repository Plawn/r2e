//! Non-regression tests for the `#[r2e::test_suite]` runtime contract:
//! **one runtime per suite**, not one per `#[case]`.
//!
//! A suite value built by `#[before_all]` outlives every case, so anything it
//! holds that is bound to a reactor — a listening socket, a spawned worker
//! task, a timer, a database pool — must still be driven when case 2 runs. With
//! a per-case runtime it is not: the resource does not error, it stops waking,
//! and the suite fails as an unexplained timeout somewhere else entirely
//! (Tasker #986). Each suite below parks a runtime-bound resource in
//! `#[before_all]` and makes several cases (and `#[after_all]`) use it.
//!
//! `#[r2e::test_suite]` resolves `r2e-test` as `crate::` inside this package
//! (proc-macro-crate reports `FoundCrate::Itself`), so the runtime-support
//! modules are re-exported at the test-crate root for the generated code to
//! find. That is the only reason the macro is usable from r2e-test's own tests.

pub use r2e_test::{ordering, suite};

use std::sync::atomic::{self, AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

use r2e::rt::io::{AsyncReadExt, AsyncWriteExt};
use r2e::rt::sync::mpsc;
use r2e::rt::{self, TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// 1. An I/O resource (a listening socket + its accept task) opened in
//    `#[before_all]` serves every case and `#[after_all]`.
// ---------------------------------------------------------------------------

struct EchoSuite {
    addr: std::net::SocketAddr,
    served: usize,
}

impl EchoSuite {
    /// Round-trip one byte through the suite-owned echo server.
    ///
    /// The timeout is the point of the test: without a shared runtime the
    /// socket's reactor is gone from case 2 on and this hangs instead of
    /// failing, exactly like the pool timeout that motivated #986.
    async fn round_trip(&mut self, byte: u8) -> u8 {
        let echoed = rt::timeout(Duration::from_secs(5), async {
            let mut stream = TcpStream::connect(self.addr).await.expect("connect");
            stream.write_all(&[byte]).await.expect("write");
            let mut buf = [0u8; 1];
            stream.read_exact(&mut buf).await.expect("read");
            buf[0]
        })
        .await
        .expect("the suite's echo server must still be driven by the suite runtime");
        self.served += 1;
        echoed
    }
}

#[r2e::test_suite(tracing = false)]
impl EchoSuite {
    #[before_all]
    async fn start_echo_server() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        // Deliberately detached: the task is owned by the suite runtime, which
        // is what must survive from one case to the next.
        rt::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                rt::spawn(async move {
                    let mut buf = [0u8; 1];
                    if stream.read_exact(&mut buf).await.is_ok() {
                        let _ = stream.write_all(&buf).await;
                    }
                });
            }
        });
        Self { addr, served: 0 }
    }

    #[case]
    async fn first_case_uses_the_socket(&mut self) {
        assert_eq!(self.round_trip(1).await, 1);
    }

    #[case]
    async fn second_case_still_uses_the_socket(&mut self) {
        assert_eq!(self.round_trip(2).await, 2);
    }

    #[case]
    async fn third_case_still_uses_the_socket(&mut self) {
        assert_eq!(self.round_trip(3).await, 3);
    }

    #[after_all]
    async fn teardown_can_still_reach_it(&mut self) {
        // Three cases have already round-tripped through the socket opened by
        // `#[before_all]`; teardown makes it four on the same reactor.
        assert_eq!(self.served, 3);
        assert_eq!(self.round_trip(4).await, 4);
        assert_eq!(self.served, 4);
    }
}

// ---------------------------------------------------------------------------
// 2. Same for a scheduler/timer-bound resource: a spawned worker behind an
//    mpsc channel, plus a `sleep` proving the timer driver is alive too.
// ---------------------------------------------------------------------------

struct WorkerSuite {
    requests: mpsc::Sender<(u32, rt::sync::oneshot::Sender<u32>)>,
    answered: usize,
}

impl WorkerSuite {
    async fn double(&mut self, n: u32) -> u32 {
        let (tx, rx) = rt::sync::oneshot::channel();
        self.requests.send((n, tx)).await.expect("worker alive");
        let answer = rt::timeout(Duration::from_secs(5), rx)
            .await
            .expect("the suite's worker task must still be driven by the suite runtime")
            .expect("worker answered");
        self.answered += 1;
        answer
    }
}

#[r2e::test_suite(tracing = false)]
impl WorkerSuite {
    #[before_all]
    async fn spawn_worker() -> Self {
        let (requests, mut rx) = mpsc::channel::<(u32, rt::sync::oneshot::Sender<u32>)>(4);
        rt::spawn(async move {
            while let Some((n, reply)) = rx.recv().await {
                // A timer inside the worker: the timer driver has to outlive
                // case 1 as well.
                rt::sleep(Duration::from_millis(1)).await;
                let _ = reply.send(n * 2);
            }
        });
        Self {
            requests,
            answered: 0,
        }
    }

    #[case]
    async fn first_case_talks_to_the_worker(&mut self) {
        assert_eq!(self.double(2).await, 4);
    }

    #[case]
    async fn second_case_talks_to_the_worker(&mut self) {
        assert_eq!(self.double(21).await, 42);
    }

    #[after_all]
    async fn teardown_talks_to_the_worker(&mut self) {
        assert_eq!(self.answered, 2);
        assert_eq!(self.double(50).await, 100);
        assert_eq!(self.answered, 3);
    }
}

// ---------------------------------------------------------------------------
// 3. A `#[should_panic]` case does not break the suite: the shared runtime and
//    the shared value survive it, later cases still run, and `#[after_all]`
//    still fires. Ordered so "panics, then another case, then teardown" is the
//    actual sequence rather than a hope.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecoverySuite {
    reached: Vec<&'static str>,
}

#[r2e::test_suite(tracing = false)]
impl RecoverySuite {
    #[case(order = 10)]
    #[should_panic(expected = "deliberate")]
    async fn panicking_case_runs_first(&mut self) {
        self.reached.push("panicking");
        // A real await first: the panic must unwind out of `block_on` without
        // taking the suite runtime with it.
        rt::sleep(Duration::from_millis(1)).await;
        panic!("deliberate");
    }

    #[case(order = 20)]
    async fn later_case_still_sees_the_suite(&mut self) {
        // The mutation from the panicking case is still there: the suite value
        // was not rebuilt, and the mutex poisoning it caused was recovered.
        assert_eq!(self.reached, ["panicking"]);
        rt::sleep(Duration::from_millis(1)).await;
        self.reached.push("later");
    }

    #[after_all]
    async fn teardown_runs_after_the_panic(&mut self) {
        assert_eq!(self.reached, ["panicking", "later"]);
        // The timer driver still works in teardown.
        rt::timeout(Duration::from_secs(5), rt::sleep(Duration::from_millis(1)))
            .await
            .expect("the suite runtime must still be driving timers in #[after_all]");
    }
}

// ---------------------------------------------------------------------------
// 4. `start_paused` is a suite-wide clock, not a fresh one per case: virtual
//    time accumulates across cases. With a per-case runtime the clock would
//    restart and `elapsed` would only ever show one case's worth.
// ---------------------------------------------------------------------------

struct PausedClockSuite {
    started: rt::Instant,
}

#[r2e::test_suite(tracing = false, flavor = "current_thread", start_paused = true)]
impl PausedClockSuite {
    #[before_all]
    async fn record_the_start() -> Self {
        Self {
            started: rt::Instant::now(),
        }
    }

    #[case(order = 10)]
    async fn first_case_advances_the_clock(&mut self) {
        rt::sleep(Duration::from_secs(3600)).await;
        assert!(self.started.elapsed() >= Duration::from_secs(3600));
    }

    #[case(order = 20)]
    async fn second_case_inherits_the_advanced_clock(&mut self) {
        rt::sleep(Duration::from_secs(3600)).await;
        assert!(
            self.started.elapsed() >= Duration::from_secs(7200),
            "the paused clock must be shared by the whole suite, not reset per case: {:?}",
            self.started.elapsed()
        );
    }

    #[after_all]
    async fn teardown_sees_the_same_clock(&mut self) {
        assert!(self.started.elapsed() >= Duration::from_secs(7200));
    }
}

// ---------------------------------------------------------------------------
// 5. The guard-rail itself: `SuiteCell` owns the runtime, and a case running
//    off it is named as such rather than left to time out.
// ---------------------------------------------------------------------------

fn multi_thread_runtime() -> rt::Runtime {
    rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn suite_cell_owns_one_runtime_shared_by_every_block_on() {
    let cell = suite::SuiteCell::<()>::new(2, multi_thread_runtime());
    let slot = cell.runtime();
    let runtime = slot.get("Suite", "case");

    // A resource created in the first `block_on` is still driven by the next
    // one — the property the whole suite form rests on.
    let listener =
        runtime.block_on(async { TcpListener::bind("127.0.0.1:0").await.expect("bind") });
    let addr = listener.local_addr().expect("local_addr");
    runtime.block_on(async move {
        rt::spawn(async move {
            let _ = listener.accept().await;
        });
    });
    runtime.block_on(async {
        rt::timeout(Duration::from_secs(5), TcpStream::connect(addr))
            .await
            .expect("listener still driven")
            .expect("connect");
    });
}

#[test]
fn guard_rail_accepts_the_suite_runtime() {
    let cell = suite::SuiteCell::<()>::new(1, multi_thread_runtime());
    let slot = cell.runtime();
    slot.get("Suite", "case")
        .block_on(async { cell.assert_on_suite_runtime("Suite", "case") });
}

#[test]
fn guard_rail_names_a_foreign_runtime() {
    let cell = suite::SuiteCell::<()>::new(1, multi_thread_runtime());

    let other = multi_thread_runtime();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        other.block_on(async { cell.assert_on_suite_runtime("EchoSuite", "second_case") })
    }))
    .expect_err("running off the suite runtime must panic");

    let message = panic_message(&panicked);
    assert!(
        message.contains("EchoSuite") && message.contains("second_case"),
        "the guard-rail must name the suite and the case: {message}"
    );
    assert!(
        message.contains("suite runtime"),
        "the guard-rail must name the cause, not just fail: {message}"
    );
}

#[test]
fn guard_rail_names_the_absence_of_a_runtime() {
    let cell = suite::SuiteCell::<()>::new(1, multi_thread_runtime());

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cell.assert_on_suite_runtime("EchoSuite", "second_case")
    }))
    .expect_err("running outside any runtime must panic");

    let message = panic_message(&panicked);
    assert!(
        message.contains("not running on any runtime"),
        "the guard-rail must say the reactor is missing: {message}"
    );
}

// ---------------------------------------------------------------------------
// 6. Teardown: the suite runtime is shut down when the suite ends, so its
//    worker threads and detached tasks do not keep running for the rest of the
//    process (the `OnceLock` holding the cell is never dropped).
// ---------------------------------------------------------------------------

/// A stand-in for a suite value that owns runtime-bound state: it records
/// whether its `Drop` saw a live reactor.
struct DropsOnItsRuntime {
    dropped_in_runtime: Arc<AtomicBool>,
}

impl Drop for DropsOnItsRuntime {
    fn drop(&mut self) {
        self.dropped_in_runtime.store(
            rt::RuntimeHandle::try_current().is_some(),
            atomic::Ordering::SeqCst,
        );
    }
}

#[test]
fn finishing_a_suite_stops_its_tasks_and_drops_the_value_on_the_runtime() {
    let cell = suite::SuiteCell::<DropsOnItsRuntime>::new(1, multi_thread_runtime());
    let ticks = Arc::new(AtomicUsize::new(0));
    let dropped_in_runtime = Arc::new(AtomicBool::new(false));

    let mut slot = cell.runtime();
    {
        let ticks = Arc::clone(&ticks);
        slot.get("TickSuite", "before_all").block_on(async move {
            // Detached, exactly like a background task a `#[before_all]` spawns.
            rt::spawn(async move {
                loop {
                    ticks.fetch_add(1, atomic::Ordering::SeqCst);
                    rt::sleep(Duration::from_millis(1)).await;
                }
            });
        });
    }

    // The task really is running before teardown.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while ticks.load(atomic::Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the suite task never started"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    slot.finish(Some(DropsOnItsRuntime {
        dropped_in_runtime: Arc::clone(&dropped_in_runtime),
    }));

    assert!(
        dropped_in_runtime.load(atomic::Ordering::SeqCst),
        "the suite value must be dropped with its reactor still under it"
    );
    assert!(!slot.is_live(), "the suite runtime must be gone");

    // …and the detached task is gone with the runtime, instead of mutating
    // shared state while unrelated tests run.
    let observed = ticks.load(atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        ticks.load(atomic::Ordering::SeqCst),
        observed,
        "the suite's detached task must not outlive the suite runtime"
    );
}

#[test]
fn finishing_twice_is_a_no_op() {
    let cell = suite::SuiteCell::<()>::new(1, multi_thread_runtime());
    let mut slot = cell.runtime();
    slot.finish(Some(()));
    slot.finish(Some(()));
    assert!(!slot.is_live());
}

#[test]
fn using_a_torn_down_suite_is_named_not_hung() {
    let cell = suite::SuiteCell::<()>::new(1, multi_thread_runtime());
    let mut slot = cell.runtime();
    slot.finish(Some(()));

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = slot.get("EchoSuite", "late_case");
    }))
    .expect_err("using a torn-down suite must panic");

    let message = panic_message(&panicked);
    assert!(
        message.contains("EchoSuite") && message.contains("late_case"),
        "the teardown guard must name the suite and the phase: {message}"
    );
    assert!(
        message.contains("torn down"),
        "the teardown guard must name the cause: {message}"
    );
}

// ---------------------------------------------------------------------------
// 7. Teardown accounting. `#[after_all]` (and with it the runtime shutdown)
//    fires on the last generated case to finish, exactly once. libtest never
//    says which cases it selected, so a filtered run legitimately does not
//    reach it — that is the documented limitation, pinned here.
// ---------------------------------------------------------------------------

fn fresh_state() -> suite::SuiteState<()> {
    suite::SuiteState {
        suite: None,
        init_failed: false,
        completed_cases: 0,
        after_all_ran: false,
    }
}

#[test]
fn teardown_fires_once_on_the_last_case() {
    let mut state = fresh_state();
    assert!(!state.complete_case(3));
    assert!(!state.complete_case(3));
    assert!(state.complete_case(3), "the last case tears the suite down");
    assert!(
        !state.complete_case(3),
        "teardown must not run a second time"
    );
}

#[test]
fn a_filtered_run_does_not_tear_the_suite_down() {
    // `cargo test one_case`: libtest runs a subset and tells nobody, so the
    // count is never reached. Documented in docs/features/12-testing.md.
    let mut state = fresh_state();
    assert!(!state.complete_case(3));
}

#[test]
fn a_single_case_suite_tears_down_immediately() {
    let mut state = fresh_state();
    assert!(state.complete_case(1));
}

// ---------------------------------------------------------------------------
// 8. A `current_thread` suite runtime is driven from whichever OS thread
//    libtest hands the case to. Deterministic here: each "case" runs on its own
//    spawned thread, and all of them share the one runtime and its resources.
// ---------------------------------------------------------------------------

#[test]
fn a_current_thread_suite_runtime_serves_cases_from_different_os_threads() {
    let runtime = rt::RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cell = Arc::new(suite::SuiteCell::<()>::new(2, runtime));

    // "before_all": an echo server owned by the suite runtime.
    let addr = {
        let slot = cell.runtime();
        slot.get("CurrentThreadSuite", "before_all")
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
                let addr = listener.local_addr().expect("local_addr");
                rt::spawn(async move {
                    while let Ok((mut stream, _)) = listener.accept().await {
                        let mut buf = [0u8; 1];
                        if stream.read_exact(&mut buf).await.is_ok() {
                            let _ = stream.write_all(&buf).await;
                        }
                    }
                });
                addr
            })
    };

    let mut thread_ids = Vec::new();
    for byte in [7u8, 9u8] {
        let cell = Arc::clone(&cell);
        thread_ids.push(
            std::thread::spawn(move || {
                let slot = cell.runtime();
                let runtime = slot.get("CurrentThreadSuite", "case");
                let echoed = runtime.block_on(async {
                    cell.assert_on_suite_runtime("CurrentThreadSuite", "case");
                    rt::timeout(Duration::from_secs(5), async {
                        let mut stream = TcpStream::connect(addr).await.expect("connect");
                        stream.write_all(&[byte]).await.expect("write");
                        let mut buf = [0u8; 1];
                        stream.read_exact(&mut buf).await.expect("read");
                        buf[0]
                    })
                    .await
                    .expect("a current_thread suite runtime must serve a case on any OS thread")
                });
                assert_eq!(echoed, byte);
                std::thread::current().id()
            })
            .join()
            .expect("case thread"),
        );
    }

    assert_ne!(
        thread_ids[0], thread_ids[1],
        "the two cases must have run on different OS threads"
    );
    assert!(!thread_ids.contains(&std::thread::current().id()));
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}
