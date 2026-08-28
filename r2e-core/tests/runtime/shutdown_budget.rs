//! Shutdown budgets: which phase each timeout bounds, and the must-run
//! guarantee for `on_stop`.
//!
//! Phase order at graceful shutdown (see `docs/features/22-serve-lifecycle.md`):
//!
//! | Phase | Bounded by |
//! |---|---|
//! | `on_drain` hooks + plugin sync shutdown | nothing |
//! | async shutdown hooks / `#[pre_destroy]` | nothing |
//! | HTTP drain (in-flight requests) | `drain_timeout` (30s by default) |
//! | tracked-handle join (`spawn_service`, `track`) | `shutdown_grace_period`, per handle |
//! | `on_stop` hooks | nothing — they **always** run |
//!
//! The single-listener tests drive `run_with_listener` on a **current-thread**
//! runtime so a thread-local `tracing` subscriber sees every event the shutdown
//! path emits (`with_default` is per thread; a multi-thread runtime would poll
//! the shutdown future on a worker that never installed it). The sharded test
//! asserts behaviour only — its workers own their own threads and runtimes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use r2e_core::builder::{AppBuilder, SpawnService};
use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::Router;
use r2e_core::rt::CancelToken;
use r2e_core::runtime::drain::DEFAULT_DRAIN_TIMEOUT;
use r2e_core::runtime::service::ServiceComponent;
use r2e_core::type_list::TNil;

// ── Log capture ─────────────────────────────────────────────────────────────

/// Every event recorded by [`CaptureLayer`], as `LEVEL field=value …` lines.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
    fn contains(&self, needle: &str) -> bool {
        self.0.lock().unwrap().iter().any(|l| l.contains(needle))
    }

    fn dump(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

struct CaptureLayer(Captured);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                let _ = write!(self.0, " {}={:?}", field.name(), value);
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                use std::fmt::Write as _;
                let _ = write!(self.0, " {}={}", field.name(), value);
            }
        }
        let mut visitor = Visitor(event.metadata().level().to_string());
        event.record(&mut visitor);
        self.0 .0.lock().unwrap().push(visitor.0);
    }
}

fn capturing() -> (Captured, impl tracing::Subscriber + Send + Sync) {
    use tracing_subscriber::layer::SubscriberExt as _;
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));
    (captured, subscriber)
}

// ── Fixtures ────────────────────────────────────────────────────────────────

fn current_thread_rt() -> r2e_core::rt::Runtime {
    r2e_core::rt::RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Router with a handler that never finishes within a test's lifetime, so the
/// HTTP drain has something to wait on.
fn slow_router() -> Router {
    Router::new().route(
        "/slow",
        get(|| async {
            r2e_core::rt::sleep(Duration::from_secs(60)).await;
            "late"
        }),
    )
}

/// Open a connection and send a complete request for `/slow`, leaving the
/// handler running (and the connection open) when this returns.
async fn hold_request_open(addr: std::net::SocketAddr) -> r2e_core::rt::TcpStream {
    use r2e_core::rt::io::AsyncWriteExt as _;
    let mut sock = r2e_core::rt::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    // Give the server time to accept, route, and enter the handler.
    r2e_core::rt::sleep(Duration::from_millis(300)).await;
    sock
}

/// A background service that **ignores** its shutdown token — the failure mode
/// `shutdown_grace_period` exists for.
struct StubbornService;

static STUBBORN_STARTED: AtomicBool = AtomicBool::new(false);

impl ServiceComponent for StubbornService {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        StubbornService
    }

    // `async fn` here would drop the trait's `+ Send` bound.
    #[allow(clippy::manual_async_fn)]
    fn start(self, _shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async {
            STUBBORN_STARTED.store(true, Ordering::SeqCst);
            // Deliberately never observes `_shutdown`.
            r2e_core::rt::sleep(Duration::from_secs(60)).await;
        }
    }
}

/// The well-behaved counterpart: returns as soon as the token fires.
struct CooperativeService;

static COOPERATIVE_STOPPED: AtomicBool = AtomicBool::new(false);

impl ServiceComponent for CooperativeService {
    type Deps = TNil;

    fn from_context(_ctx: &r2e_core::beans::BeanContext) -> Self {
        CooperativeService
    }

    #[allow(clippy::manual_async_fn)]
    fn start(self, shutdown: CancelToken) -> impl std::future::Future<Output = ()> + Send {
        async move {
            shutdown.cancelled().await;
            COOPERATIVE_STOPPED.store(true, Ordering::SeqCst);
        }
    }
}

// ── Resolving the drain budget: builder > config > 30s default ──────────────

/// Build a stateless app on top of a YAML config and read back the budget it
/// resolved — the same trick `runtime::tcp_nodelay` uses, so no test here has
/// to sit out a real 30-second drain.
fn prepare_with_yaml(yaml: &str) -> r2e_core::builder::PreparedApp<()> {
    // `load_config` mutates process-global dev-reload state (see dev_serial).
    let _serial = crate::dev_serial::dev_serial();
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .with_state(())
        .prepare("0.0.0.0:0")
}

#[test]
fn drain_is_bounded_at_thirty_seconds_by_default() {
    assert_eq!(DEFAULT_DRAIN_TIMEOUT, Duration::from_secs(30));

    let app = AppBuilder::new().with_state(()).prepare("0.0.0.0:0");
    assert_eq!(
        app.drain_timeout(),
        Some(Duration::from_secs(30)),
        "an app that never heard of drain_timeout must still be bounded"
    );
}

#[test]
fn drain_timeout_defaults_when_the_config_key_is_absent() {
    let app = prepare_with_yaml("server:\n  port: 3000\n");
    assert_eq!(app.drain_timeout(), Some(DEFAULT_DRAIN_TIMEOUT));
}

#[test]
fn config_drain_timeout_is_honored() {
    let app = prepare_with_yaml("server:\n  drain-timeout: 5s\n");
    assert_eq!(app.drain_timeout(), Some(Duration::from_secs(5)));

    // The plain-integer form is seconds, like every other duration key.
    let app = prepare_with_yaml("server:\n  drain-timeout: 7\n");
    assert_eq!(app.drain_timeout(), Some(Duration::from_secs(7)));

    // …and sub-second units survive the round trip.
    let app = prepare_with_yaml("server:\n  drain-timeout: 250ms\n");
    assert_eq!(app.drain_timeout(), Some(Duration::from_millis(250)));
}

#[test]
fn builder_drain_timeout_wins_over_config() {
    let _serial = crate::dev_serial::dev_serial();
    let config = R2eConfig::from_yaml_str("server:\n  drain-timeout: 5s\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .with_state(())
        .drain_timeout(Duration::from_secs(11))
        .prepare("0.0.0.0:0");
    assert_eq!(app.drain_timeout(), Some(Duration::from_secs(11)));
}

#[test]
fn drain_timeout_unbounded_yields_no_bound() {
    let app = AppBuilder::new()
        .with_state(())
        .drain_timeout_unbounded()
        .prepare("0.0.0.0:0");
    assert_eq!(
        app.drain_timeout(),
        None,
        "the explicit opt-out is the only way back to an unbounded drain"
    );

    // It also wins over a configured budget: unbounded is a code decision.
    let _serial = crate::dev_serial::dev_serial();
    let config = R2eConfig::from_yaml_str("server:\n  drain-timeout: 5s\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .with_state(())
        .drain_timeout_unbounded()
        .prepare("0.0.0.0:0");
    assert_eq!(app.drain_timeout(), None);
}

#[test]
fn invalid_config_drain_timeout_falls_back_to_the_default() {
    // An unparseable budget must never silently mean "unbounded".
    let app = prepare_with_yaml("server:\n  drain-timeout: nonsense\n");
    assert_eq!(app.drain_timeout(), Some(DEFAULT_DRAIN_TIMEOUT));
}

// ── `drain_timeout` bounds the HTTP drain ───────────────────────────────────

#[test]
fn drain_timeout_bounds_the_http_drain_and_on_stop_still_runs() {
    let (captured, subscriber) = capturing();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_hook = stopped.clone();

    let rt = current_thread_rt();
    let elapsed = tracing::subscriber::with_default(subscriber, || {
        rt.block_on(async move {
            let listener = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let app = AppBuilder::new()
                .with_state(())
                .register_routes(slow_router())
                .drain_timeout(Duration::from_millis(300))
                .on_stop(move |_state| {
                    let stopped = stopped_hook.clone();
                    async move {
                        stopped.store(true, Ordering::SeqCst);
                    }
                })
                .prepare(&addr.to_string());
            let stop = app.stop_handle();
            let server = r2e_core::rt::spawn(async move {
                app.run_with_listener(listener)
                    .await
                    .map_err(|e| e.to_string())
            });

            let _sock = hold_request_open(addr).await;

            let started = Instant::now();
            stop.stop();
            let joined = r2e_core::rt::timeout(Duration::from_secs(15), server).await;
            let elapsed = started.elapsed();
            match joined {
                Ok(Ok(Ok(()))) => {}
                other => panic!("server did not stop cleanly: {other:?}"),
            }
            elapsed
        })
    });

    assert!(
        elapsed < Duration::from_secs(10),
        "shutdown must not wait for the 60s handler: took {elapsed:?}"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "on_stop must run even after a drain timeout"
    );
    assert!(
        captured.contains("drain_timeout elapsed"),
        "expected a drain-timeout warning, got:\n{}",
        captured.dump()
    );
}

// ── `shutdown_grace_period` bounds the tracked-handle join ──────────────────

#[test]
fn grace_period_bounds_a_stubborn_service_and_names_it() {
    let (captured, subscriber) = capturing();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_hook = stopped.clone();
    STUBBORN_STARTED.store(false, Ordering::SeqCst);

    let rt = current_thread_rt();
    let elapsed = tracing::subscriber::with_default(subscriber, || {
        rt.block_on(async move {
            let listener = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let app = AppBuilder::new()
                .with_state(())
                .spawn_service::<StubbornService>()
                .shutdown_grace_period(Duration::from_millis(300))
                .on_stop(move |_state| {
                    let stopped = stopped_hook.clone();
                    async move {
                        stopped.store(true, Ordering::SeqCst);
                    }
                })
                .prepare(&addr.to_string());
            let stop = app.stop_handle();
            let server = r2e_core::rt::spawn(async move {
                app.run_with_listener(listener)
                    .await
                    .map_err(|e| e.to_string())
            });

            r2e_core::rt::sleep(Duration::from_millis(300)).await;
            assert!(
                STUBBORN_STARTED.load(Ordering::SeqCst),
                "the service task must have started before we stop"
            );

            let started = Instant::now();
            stop.stop();
            let joined = r2e_core::rt::timeout(Duration::from_secs(15), server).await;
            let elapsed = started.elapsed();
            match joined {
                Ok(Ok(Ok(()))) => {}
                other => panic!("server did not stop cleanly: {other:?}"),
            }
            elapsed
        })
    });

    assert!(
        elapsed < Duration::from_secs(10),
        "shutdown must not wait for the 60s service: took {elapsed:?}"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "on_stop must run even after the grace period is exhausted"
    );
    assert!(
        captured.contains("shutdown_grace_period elapsed"),
        "expected a grace-period warning, got:\n{}",
        captured.dump()
    );
    assert!(
        captured.contains("StubbornService"),
        "the warning must name the offending service, got:\n{}",
        captured.dump()
    );
}

// ── Nominal path: no budget is hit, no warning is emitted ───────────────────

#[test]
fn cooperative_shutdown_emits_no_budget_warnings() {
    let (captured, subscriber) = capturing();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_hook = stopped.clone();
    COOPERATIVE_STOPPED.store(false, Ordering::SeqCst);

    let rt = current_thread_rt();
    tracing::subscriber::with_default(subscriber, || {
        rt.block_on(async move {
            let listener = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let app = AppBuilder::new()
                .with_state(())
                .register_routes(Router::new().route("/ping", get(|| async { "pong" })))
                .spawn_service::<CooperativeService>()
                // Configured but generous: neither budget should be reached.
                .drain_timeout(Duration::from_secs(30))
                .shutdown_grace_period(Duration::from_secs(30))
                .on_stop(move |_state| {
                    let stopped = stopped_hook.clone();
                    async move {
                        stopped.store(true, Ordering::SeqCst);
                    }
                })
                .prepare(&addr.to_string());
            let stop = app.stop_handle();
            let server = r2e_core::rt::spawn(async move {
                app.run_with_listener(listener)
                    .await
                    .map_err(|e| e.to_string())
            });

            r2e_core::rt::sleep(Duration::from_millis(200)).await;
            stop.stop();
            let joined = r2e_core::rt::timeout(Duration::from_secs(15), server).await;
            match joined {
                Ok(Ok(Ok(()))) => {}
                other => panic!("server did not stop cleanly: {other:?}"),
            }
        })
    });

    assert!(stopped.load(Ordering::SeqCst), "on_stop must run");
    assert!(
        COOPERATIVE_STOPPED.load(Ordering::SeqCst),
        "the cooperative service must have observed its token"
    );
    assert!(
        !captured.contains("drain_timeout elapsed"),
        "no drain warning expected, got:\n{}",
        captured.dump()
    );
    assert!(
        !captured.contains("shutdown_grace_period elapsed"),
        "no grace-period warning expected, got:\n{}",
        captured.dump()
    );
}

// ── Sharded strategy behaves the same ───────────────────────────────────────

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod sharded {
    use super::*;
    use r2e_core::config::R2eConfig;

    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        port
    }

    /// Same guarantee as the single-listener path: each worker bounds its own
    /// drain, so the whole set finishes within `drain_timeout` of the signal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_timeout_bounds_each_worker_and_on_stop_still_runs() {
        let port = free_port();
        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let yaml = format!("server:\n  workers: 2\n  port: {port}\n");
        let config = R2eConfig::from_yaml_str(&yaml).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_hook = stopped.clone();

        let serial = crate::dev_serial::dev_serial();
        let app = AppBuilder::new()
            .override_config(config)
            .load_config::<()>()
            .with_state(())
            .register_routes(slow_router())
            .drain_timeout(Duration::from_millis(300))
            .on_stop(move |_state| {
                let stopped = stopped_hook.clone();
                async move {
                    stopped.store(true, Ordering::SeqCst);
                }
            })
            .prepare(&addr.to_string());
        drop(serial);
        let stop = app.stop_handle();
        let server = r2e_core::rt::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

        // Wait for the workers to bind, then pin one of them on a slow handler.
        let mut sock = None;
        for _ in 0..200 {
            if let Ok(s) = r2e_core::rt::TcpStream::connect(addr).await {
                sock = Some(s);
                break;
            }
            r2e_core::rt::sleep(Duration::from_millis(25)).await;
        }
        let mut sock = sock.expect("sharded server never accepted a connection");
        {
            use r2e_core::rt::io::AsyncWriteExt as _;
            sock.write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            sock.flush().await.unwrap();
        }
        r2e_core::rt::sleep(Duration::from_millis(300)).await;

        let started = Instant::now();
        stop.stop();
        let joined = r2e_core::rt::timeout(Duration::from_secs(15), server).await;
        let elapsed = started.elapsed();
        match joined {
            Ok(Ok(Ok(()))) => {}
            other => panic!("sharded server did not stop cleanly: {other:?}"),
        }
        assert!(
            elapsed < Duration::from_secs(10),
            "sharded shutdown must not wait for the 60s handler: took {elapsed:?}"
        );
        assert!(
            stopped.load(Ordering::SeqCst),
            "on_stop must run after a sharded drain timeout"
        );
        drop(sock);
    }
}
