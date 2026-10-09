//! `StopPhase::AfterDrain` (#1071): a service that *consumes* what the
//! request path produces must outlive the HTTP drain.
//!
//! Shutdown order with both lanes populated:
//!
//! | Step | Early lane | After-drain lane |
//! |---|---|---|
//! | 2. plugin sync hooks | **cancelled** | untouched |
//! | 3. HTTP drain | — | still consuming |
//! | 4. tracked-handle join | joined | untouched |
//! | 5. after-drain stop | — | **cancelled + joined, one at a time in `stop_order`** |
//! | 6. `on_stop` | — | — |
//!
//! The serving test runs on a current-thread runtime for the same reason the
//! budget tests do (see `shutdown_budget.rs`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use r2e_core::builder::{AppBuilder, SpawnService};
use r2e_core::http::routing::get;
use r2e_core::http::Router;
use r2e_core::rt::sync::mpsc;
use r2e_core::rt::CancelToken;
use r2e_core::runtime::service::ServiceComponent;
use r2e_core::type_list::TNil;
use r2e_core::StopPhase;

use super::shutdown_budget::current_thread_rt;

// ── Event log ───────────────────────────────────────────────────────────────

/// Ordered record of what happened, per test (each test owns its own log so
/// the target can run them in parallel).
struct Events(Mutex<Vec<&'static str>>);

impl Events {
    const fn new() -> Self {
        Events(Mutex::new(Vec::new()))
    }
    fn push(&self, e: &'static str) {
        self.0.lock().unwrap().push(e);
    }
    fn snapshot(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
    fn reset(&self) {
        self.0.lock().unwrap().clear();
    }
    /// Index of `e` in the log, or a panic naming the whole log.
    fn index_of(&self, e: &str) -> usize {
        let log = self.snapshot();
        log.iter()
            .position(|x| *x == e)
            .unwrap_or_else(|| panic!("`{e}` never happened; log: {log:?}"))
    }
}

// ── Serving: a sink fed by a handler survives the HTTP drain ───────────────

static SERVE_EVENTS: Events = Events::new();
/// The sink's receiving end, parked here so the service (constructed by type,
/// without arguments) can pick it up when it starts.
static SINK_RX: Mutex<Option<mpsc::UnboundedReceiver<u32>>> = Mutex::new(None);
static SINK_TX: Mutex<Option<mpsc::UnboundedSender<u32>>> = Mutex::new(None);

/// Consumes what `/slow-write` produces. `AfterDrain`: must still be running
/// when the handler, which finishes *during* the drain, sends its item.
struct WriteBehindSink;

impl ServiceComponent for WriteBehindSink {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        WriteBehindSink
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            let mut rx = SINK_RX.lock().unwrap().take().expect("sink receiver");
            loop {
                r2e_core::rt::select! {
                    biased;
                    item = rx.recv() => match item {
                        Some(_) => SERVE_EVENTS.push("sink-processed"),
                        None => break,
                    },
                    _ = shutdown.cancelled() => {
                        SERVE_EVENTS.push("sink-cancelled");
                        break;
                    }
                }
            }
        }
    }
}

/// The control: a default (`Early`) service, cancelled at step 2 — i.e.
/// before the handler has even finished.
struct EarlyProbe;

impl ServiceComponent for EarlyProbe {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        EarlyProbe
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            shutdown.cancelled().await;
            SERVE_EVENTS.push("early-cancelled");
        }
    }
}

/// A handler that is still running when shutdown starts and hands its result
/// to the sink just before returning — the write-behind shape of the ticket.
fn write_behind_router() -> Router {
    Router::new().route(
        "/slow-write",
        get(|| async {
            r2e_core::rt::sleep(Duration::from_millis(500)).await;
            let tx = SINK_TX.lock().unwrap().clone().expect("sink sender");
            tx.send(1)
                .expect("the sink must still be alive during the drain");
            SERVE_EVENTS.push("request-done");
            "written"
        }),
    )
}

async fn send_request_and_leave_it_running(addr: std::net::SocketAddr) -> r2e_core::rt::TcpStream {
    use r2e_core::rt::io::AsyncWriteExt as _;
    let mut sock = r2e_core::rt::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /slow-write HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    r2e_core::rt::sleep(Duration::from_millis(200)).await;
    sock
}

#[test]
fn an_after_drain_sink_consumes_what_an_in_flight_request_produced() {
    SERVE_EVENTS.reset();
    let (tx, rx) = mpsc::unbounded_channel();
    *SINK_RX.lock().unwrap() = Some(rx);
    *SINK_TX.lock().unwrap() = Some(tx);

    let rt = current_thread_rt();
    rt.block_on(async move {
        let listener = r2e_core::rt::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        let app = AppBuilder::new()
            .with_state(())
            .register_routes(write_behind_router())
            .spawn_service::<EarlyProbe>()
            .spawn_service::<WriteBehindSink>()
            .on_stop(|_state| async { SERVE_EVENTS.push("on_stop") })
            .prepare(&addr.to_string());
        let stop = app.stop_handle();
        let server = r2e_core::rt::spawn(async move {
            app.run_with_listener(listener)
                .await
                .map_err(|e| e.to_string())
        });

        let _sock = send_request_and_leave_it_running(addr).await;
        stop.stop();
        match r2e_core::rt::timeout(Duration::from_secs(15), server).await {
            Ok(Ok(Ok(()))) => {}
            other => panic!("server did not stop cleanly: {other:?}"),
        }
    });

    let log = SERVE_EVENTS.snapshot();
    let early = SERVE_EVENTS.index_of("early-cancelled");
    let request = SERVE_EVENTS.index_of("request-done");
    let processed = SERVE_EVENTS.index_of("sink-processed");
    let cancelled = SERVE_EVENTS.index_of("sink-cancelled");
    let on_stop = SERVE_EVENTS.index_of("on_stop");
    assert!(
        early < request,
        "an Early service is cancelled at step 2, before the in-flight request finishes: {log:?}"
    );
    assert!(
        request < processed && processed < cancelled,
        "the sink must consume the item the draining request produced BEFORE it is told \
         to stop: {log:?}"
    );
    assert!(
        cancelled < on_stop,
        "after-drain services are joined before `on_stop`: {log:?}"
    );
    // Nothing on the sink lane may have observed the step-3 root cancellation
    // early: exactly one cancel, after exactly one processed item.
    assert_eq!(
        log.iter().filter(|e| **e == "sink-cancelled").count(),
        1,
        "{log:?}"
    );
}

// ── Dropping the `run()` future still cancels an after-drain service ────────

static DROP_STARTED: AtomicBool = AtomicBool::new(false);
static DROP_STOPPED: AtomicBool = AtomicBool::new(false);

struct DropProbeSink;

impl ServiceComponent for DropProbeSink {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        DropProbeSink
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            DROP_STARTED.store(true, Ordering::SeqCst);
            shutdown.cancelled().await;
            DROP_STOPPED.store(true, Ordering::SeqCst);
        }
    }
}

/// The after-drain root is NOT a child of the app root (step 3 must not reach
/// it), so it needs its own cancel-on-drop guard for the paths where no stop
/// sequence runs at all — an `r2e dev` hot patch dropping the whole future.
#[tokio::test]
async fn dropping_the_run_future_cancels_an_after_drain_service() {
    let app = AppBuilder::new()
        .with_state(())
        .spawn_service::<DropProbeSink>();
    let prepared = app.prepare("127.0.0.1:0");
    let listener = r2e_core::rt::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();

    let mut server = Box::pin(prepared.run_with_listener(listener));
    r2e_core::rt::select! {
        r = &mut server => panic!("the server returned before the service started: {r:?}"),
        _ = r2e_core::rt::sleep(Duration::from_millis(100)) => {}
    }
    assert!(DROP_STARTED.load(Ordering::SeqCst));

    drop(server);

    r2e_core::rt::timeout(Duration::from_secs(3), async {
        while !DROP_STOPPED.load(Ordering::SeqCst) {
            r2e_core::rt::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the run() future must cancel an after-drain service's token too");
}

// ── In-process: `stop_order` is sequential, not merely sorted ───────────────

static ORDER_EVENTS: Events = Events::new();

/// Order 1: told to stop first, and deliberately slow to finish — the test is
/// that order 2 is NOT cancelled until this one has fully returned.
struct FirstToStop;

impl ServiceComponent for FirstToStop {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        FirstToStop
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    fn stop_order() -> i32 {
        1
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            shutdown.cancelled().await;
            ORDER_EVENTS.push("first-cancelled");
            r2e_core::rt::sleep(Duration::from_millis(150)).await;
            ORDER_EVENTS.push("first-done");
        }
    }
}

struct SecondToStop;

impl ServiceComponent for SecondToStop {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        SecondToStop
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    fn stop_order() -> i32 {
        2
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            shutdown.cancelled().await;
            ORDER_EVENTS.push("second-cancelled");
        }
    }
}

#[tokio::test]
async fn after_drain_services_stop_one_at_a_time_in_stop_order() {
    ORDER_EVENTS.reset();
    // Registered in REVERSE order on purpose: the stop order must come from
    // `stop_order`, not from registration.
    let app = AppBuilder::new()
        .with_state(())
        .spawn_service::<SecondToStop>()
        .spawn_service::<FirstToStop>()
        .on_stop(|_state| async { ORDER_EVENTS.push("on_stop") })
        .prepare("127.0.0.1:0")
        .start_in_process()
        .await
        .expect("boot");
    // Let both tasks reach their `cancelled()` await.
    r2e_core::rt::sleep(Duration::from_millis(50)).await;
    assert!(
        app.has_shutdown_work(),
        "two live after-drain services are shutdown work"
    );

    app.shutdown().await;

    let log = ORDER_EVENTS.snapshot();
    assert_eq!(
        log,
        vec![
            "first-cancelled",
            "first-done",
            "second-cancelled",
            "on_stop"
        ],
        "order 1 must be cancelled AND joined before order 2 is cancelled"
    );
}

// ── In-process drop: after-drain tasks are aborted, not detached ────────────

static ABORT_STARTED: AtomicBool = AtomicBool::new(false);
static ABORT_FUTURE_DROPPED: AtomicBool = AtomicBool::new(false);

/// Lives inside the service future: dropped only when the future is — i.e.
/// when the task is aborted (or finishes, which this one never does).
struct DropFlag;

impl Drop for DropFlag {
    fn drop(&mut self) {
        ABORT_FUTURE_DROPPED.store(true, Ordering::SeqCst);
    }
}

struct IgnoresItsToken;

impl ServiceComponent for IgnoresItsToken {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        IgnoresItsToken
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, _shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            let _flag = DropFlag;
            ABORT_STARTED.store(true, Ordering::SeqCst);
            r2e_core::rt::sleep(Duration::from_secs(60)).await;
        }
    }
}

/// `RunningApp::drop` without `shutdown()` (a test that forgot, a panic) must
/// not leak after-drain tasks into the runtime: they are aborted like the
/// tracked lane is.
#[tokio::test]
async fn dropping_a_running_app_aborts_its_after_drain_services() {
    let app = AppBuilder::new()
        .with_state(())
        .spawn_service::<IgnoresItsToken>()
        .prepare("127.0.0.1:0")
        .start_in_process()
        .await
        .expect("boot");
    r2e_core::rt::sleep(Duration::from_millis(50)).await;
    assert!(ABORT_STARTED.load(Ordering::SeqCst));
    assert!(app.has_shutdown_work());
    assert!(!ABORT_FUTURE_DROPPED.load(Ordering::SeqCst));

    drop(app);

    r2e_core::rt::timeout(Duration::from_secs(3), async {
        while !ABORT_FUTURE_DROPPED.load(Ordering::SeqCst) {
            r2e_core::rt::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the RunningApp must abort (not detach) an after-drain service task");
}

// ── Derive: `#[service(stop = "after_drain", order = N)]` ──────────────────

#[derive(r2e_macros::BackgroundService)]
#[service(stop = "after_drain", order = 3)]
struct DerivedSink;

impl DerivedSink {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

#[derive(r2e_macros::BackgroundService)]
struct DerivedDefault;

impl DerivedDefault {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

#[test]
fn the_derive_emits_the_declared_stop_phase_and_order() {
    assert_eq!(DerivedSink::stop_phase(), StopPhase::AfterDrain);
    assert_eq!(DerivedSink::stop_order(), 3);
    assert_eq!(DerivedDefault::stop_phase(), StopPhase::Early);
    assert_eq!(DerivedDefault::stop_order(), 0);
}
