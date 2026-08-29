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
// 3. The guard-rail itself: `SuiteCell` owns the runtime, and a case running
//    off it is named as such rather than left to time out.
// ---------------------------------------------------------------------------

#[test]
fn suite_cell_owns_one_runtime_shared_by_every_block_on() {
    let runtime = rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cell = suite::SuiteCell::<()>::new(2, runtime);

    // A resource created in the first `block_on` is still driven by the next
    // one — the property the whole suite form rests on.
    let listener = cell
        .runtime()
        .block_on(async { TcpListener::bind("127.0.0.1:0").await.expect("bind") });
    let addr = listener.local_addr().expect("local_addr");
    cell.runtime().block_on(async move {
        rt::spawn(async move {
            let _ = listener.accept().await;
        });
    });
    cell.runtime().block_on(async {
        rt::timeout(Duration::from_secs(5), TcpStream::connect(addr))
            .await
            .expect("listener still driven")
            .expect("connect");
    });
}

#[test]
fn guard_rail_accepts_the_suite_runtime() {
    let runtime = rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cell = suite::SuiteCell::<()>::new(1, runtime);
    cell.runtime()
        .block_on(async { cell.assert_on_suite_runtime("Suite", "case") });
}

#[test]
fn guard_rail_names_a_foreign_runtime() {
    let suite_runtime = rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cell = suite::SuiteCell::<()>::new(1, suite_runtime);

    let other = rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
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
    let runtime = rt::RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cell = suite::SuiteCell::<()>::new(1, runtime);

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

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}
