//! Tests for per-worker services (`AppBuilder::per_worker_service`,
//! `r2e_core::runtime::worker`).
//!
//! The lifecycle tests drive `serve_sharded` directly (like the port-0 test in
//! `sharded.rs`) so they can cancel via the token instead of raising SIGINT;
//! the builder-level tests go through `PreparedApp::run` + `StopHandle`.

use r2e_core::builder::AppBuilder;
use r2e_core::runtime::worker::{BoxError, WorkerContext};

// ── Builder-level: registration requires sharding ───────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_worker_service_without_workers_is_a_run_error() {
    let app = AppBuilder::new()
        .with_state(())
        .per_worker_service(|_w: WorkerContext| async move { Ok::<(), BoxError>(()) })
        .prepare("127.0.0.1:0");
    let err = app
        .run()
        .await
        .expect_err("run() must reject per-worker services without server.workers");
    assert!(
        err.to_string().contains("server.workers"),
        "unexpected error: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_worker_service_with_explicit_listener_is_an_error() {
    let listener = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
    let app = AppBuilder::new()
        .with_state(())
        .per_worker_service(|_w: WorkerContext| async move { Ok::<(), BoxError>(()) })
        .prepare("127.0.0.1:0");
    let err = app
        .run_with_listener(listener)
        .await
        .expect_err("run_with_listener must reject per-worker services");
    assert!(
        err.to_string().contains("run_with_listener"),
        "unexpected error: {err}"
    );
}

/// `dev-reload` forces the single cached-listener path (hot-reload + sharding
/// is unsupported in v1, `docs/features/19-sharded-serving.md` § Limitations),
/// so there are no worker runtimes for a per-worker service to live on.
/// Running the factory on the multi-thread control plane instead would break
/// the `!Send` ownership promise silently, so `run()` must refuse — never fall
/// back. This is the feature-on counterpart of
/// `lifecycle::builder_per_worker_service_runs_and_stops_via_stop_handle`.
#[cfg(feature = "dev-reload")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_worker_service_under_dev_reload_is_a_run_error() {
    let config = r2e_core::config::R2eConfig::from_yaml_str("server:\n  workers: 2\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .with_state(())
        .per_worker_service(|_w: WorkerContext| async move { Ok::<(), BoxError>(()) })
        .prepare("127.0.0.1:0");
    let err = app
        .run()
        .await
        .expect_err("run() must reject per-worker services under dev-reload");
    let msg = err.to_string();
    assert!(
        msg.contains("dev-reload") && msg.contains("per_worker_service"),
        "the error must name both the feature and the offending call: {msg}"
    );
}

// ── Lifecycle (supported platforms only) ────────────────────────────────────

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod lifecycle {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;
    use std::time::Duration;

    // Only the builder-level end-to-end test needs these, and that test is
    // compiled out under `dev-reload` (see its doc comment).
    #[cfg(not(feature = "dev-reload"))]
    use r2e_core::config::R2eConfig;
    use r2e_core::rt::CancelToken;
    use r2e_core::runtime::sharded::{serve_sharded, WorkerPark};
    use r2e_core::runtime::worker::{
        BoxError, LocalBoxFuture, PerWorkerServiceFactory, WorkerContext, WorkerService,
    };

    #[cfg(not(feature = "dev-reload"))]
    use super::AppBuilder;

    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        port
    }

    /// Blocking HTTP/1.1 `GET /ping`; `Ok` when the server answered `200`.
    fn ping(addr: &str) -> Result<(), String> {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect_timeout(
            &addr.parse().unwrap(),
            Duration::from_millis(500),
        )
        .map_err(|e| format!("connect: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .map_err(|e| format!("write: {e}"))?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
        let text = String::from_utf8_lossy(&buf);
        if text.starts_with("HTTP/1.1 200") && text.contains("pong") {
            Ok(())
        } else {
            Err(format!("bad response: {text:?}"))
        }
    }

    fn wait_ready(addr: &str) {
        for _ in 0..200 {
            if ping(addr).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("server at {addr} did not become ready");
    }

    fn router() -> r2e_core::http::Router {
        use r2e_core::http::routing::get;
        r2e_core::http::Router::new()
            .route("/ping", get(|| async { "pong" }))
            .with_state(())
    }

    /// Run `serve_sharded` on a helper thread with the given factories.
    /// `while_serving` runs on the caller once the server answers `/ping`
    /// (skipped when `expect_ready` is false — startup-failure cases never
    /// become ready); then the token is cancelled and the result returned.
    fn run_sharded(
        workers: usize,
        factories: Vec<PerWorkerServiceFactory>,
        expect_ready: bool,
        while_serving: impl FnOnce(&str),
    ) -> Result<(), String> {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let token = CancelToken::new();
        let cancel = token.clone();
        let cp_rt = r2e_core::rt::RuntimeBuilder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let cp_handle = cp_rt.handle();
        let bind_addr: std::net::SocketAddr = addr.parse().unwrap();
        let handle = std::thread::spawn(move || {
            serve_sharded(
                router(),
                &[bind_addr],
                workers,
                true,
                cp_handle,
                token,
                None,
                &factories,
                WorkerPark::unparked(),
                r2e_core::runtime::worker_set::WorkerSet::new(),
            )
            .map_err(|e| e.to_string())
        });
        if expect_ready {
            wait_ready(&addr);
            while_serving(&addr);
            cancel.cancel();
        }
        // Startup-failure runs return on their own; bound the wait either way.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !handle.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "serve_sharded did not return within 15s"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().expect("serve_sharded thread panicked")
    }

    /// What one factory invocation observed.
    #[derive(Debug, Clone)]
    struct Seen {
        id: usize,
        workers: usize,
        thread: ThreadId,
        thread_name: Option<String>,
    }

    fn recording_factory(seen: Arc<Mutex<Vec<Seen>>>) -> PerWorkerServiceFactory {
        PerWorkerServiceFactory::new(move |w: WorkerContext| {
            let seen = seen.clone();
            async move {
                // Runs on the worker thread: the context's thread id IS ours.
                assert_eq!(w.thread_id(), std::thread::current().id());
                seen.lock().unwrap().push(Seen {
                    id: w.id(),
                    workers: w.workers(),
                    thread: std::thread::current().id(),
                    thread_name: std::thread::current().name().map(str::to_owned),
                });
                Ok::<(), BoxError>(())
            }
        })
    }

    fn assert_exactly_once_per_worker(n: usize) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let res = run_sharded(n, vec![recording_factory(seen.clone())], true, |addr| {
            for _ in 0..5 {
                ping(addr).unwrap();
            }
        });
        res.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), n, "factory must run exactly once per worker: {seen:?}");
        let ids: BTreeSet<usize> = seen.iter().map(|s| s.id).collect();
        assert_eq!(ids, (0..n).collect::<BTreeSet<_>>(), "ids must be 0..n");
        let threads: std::collections::HashSet<ThreadId> = seen.iter().map(|s| s.thread).collect();
        assert_eq!(threads.len(), n, "each worker runs on its own thread");
        for s in seen.iter() {
            assert_eq!(s.workers, n);
            assert_eq!(s.thread_name.as_deref(), Some(format!("r2e-worker-{}", s.id).as_str()));
        }
    }

    #[test]
    fn factory_runs_exactly_once_per_worker_1() {
        assert_exactly_once_per_worker(1);
    }

    #[test]
    fn factory_runs_exactly_once_per_worker_2() {
        assert_exactly_once_per_worker(2);
    }

    #[test]
    fn factory_runs_exactly_once_per_worker_4() {
        assert_exactly_once_per_worker(4);
    }

    /// A `!Send` service: `Rc<RefCell<_>>` state shared with a local task that
    /// lives until the worker's shutdown token fires. `shutdown` awaits that
    /// task, proving (a) local tasks execute on the worker, (b) the shutdown
    /// token is signalled before `WorkerService::shutdown`, (c) the service is
    /// still alive (not dropped) while its shutdown future runs.
    struct RcService {
        state: Rc<RefCell<u64>>,
        watcher: Option<r2e_core::rt::JobHandle<u64>>,
        report: Arc<Mutex<Vec<(usize, u64)>>>,
        id: usize,
    }

    impl WorkerService for RcService {
        fn shutdown(mut self: Box<Self>) -> LocalBoxFuture<'static, ()> {
            Box::pin(async move {
                let observed = self.watcher.take().unwrap().await.unwrap();
                assert_eq!(observed, *self.state.borrow(), "watcher saw the shared Rc state");
                self.report.lock().unwrap().push((self.id, *self.state.borrow()));
            })
        }
    }

    #[test]
    fn not_send_state_and_local_tasks_stay_on_the_worker() {
        let report = Arc::new(Mutex::new(Vec::new()));
        let cancelled_before_shutdown = Arc::new(AtomicUsize::new(0));
        let factory = {
            let report = report.clone();
            let cancelled_before_shutdown = cancelled_before_shutdown.clone();
            PerWorkerServiceFactory::new(move |w: WorkerContext| {
                let report = report.clone();
                let flag = cancelled_before_shutdown.clone();
                async move {
                    let state = Rc::new(RefCell::new(0u64));
                    // A short local task mutating the Rc: must complete on this
                    // thread before the factory returns.
                    let s2 = Rc::clone(&state);
                    w.spawn_local(async move { *s2.borrow_mut() += 10 })
                        .await
                        .unwrap();
                    assert_eq!(*state.borrow(), 10);
                    // A long-lived local task: waits for the worker shutdown
                    // signal, then reads the Rc one last time.
                    let s3 = Rc::clone(&state);
                    let shutdown = w.shutdown();
                    let watcher = w.spawn_local(async move {
                        shutdown.cancelled().await;
                        flag.fetch_add(1, Ordering::SeqCst);
                        *s3.borrow_mut() += 1;
                        *s3.borrow()
                    });
                    Ok::<_, BoxError>(RcService {
                        state,
                        watcher: Some(watcher),
                        report,
                        id: w.id(),
                    })
                }
            })
        };
        run_sharded(2, vec![factory], true, |addr| ping(addr).unwrap()).unwrap();
        let mut report = report.lock().unwrap().clone();
        report.sort();
        assert_eq!(report, vec![(0, 11), (1, 11)]);
        assert_eq!(cancelled_before_shutdown.load(Ordering::SeqCst), 2);
    }

    struct Counting {
        shut: Arc<Mutex<Vec<(usize, usize)>>>,
        worker: usize,
        idx: usize,
    }

    impl WorkerService for Counting {
        fn shutdown(self: Box<Self>) -> LocalBoxFuture<'static, ()> {
            Box::pin(async move {
                r2e_core::rt::yield_now().await;
                self.shut.lock().unwrap().push((self.worker, self.idx));
            })
        }
    }

    fn counting_factory(idx: usize, shut: Arc<Mutex<Vec<(usize, usize)>>>) -> PerWorkerServiceFactory {
        PerWorkerServiceFactory::new(move |w: WorkerContext| {
            let shut = shut.clone();
            async move {
                Ok::<_, BoxError>(Counting {
                    shut,
                    worker: w.id(),
                    idx,
                })
            }
        })
    }

    #[test]
    fn graceful_shutdown_runs_every_service_in_reverse_order() {
        let shut = Arc::new(Mutex::new(Vec::new()));
        run_sharded(
            4,
            vec![counting_factory(0, shut.clone()), counting_factory(1, shut.clone())],
            true,
            |addr| ping(addr).unwrap(),
        )
        .unwrap();
        let shut = shut.lock().unwrap();
        assert_eq!(shut.len(), 8, "2 services × 4 workers must all shut down: {shut:?}");
        for w in 0..4 {
            let order: Vec<usize> = shut.iter().filter(|(ww, _)| *ww == w).map(|(_, i)| *i).collect();
            assert_eq!(order, vec![1, 0], "worker {w} must shut down in reverse start order");
        }
    }

    #[test]
    fn startup_failure_names_the_worker_and_unwinds_started_services() {
        let shut = Arc::new(Mutex::new(Vec::new()));
        let served = Arc::new(AtomicBool::new(false));
        let failing = PerWorkerServiceFactory::new(|w: WorkerContext| async move {
            if w.id() == 2 {
                Err::<(), BoxError>("disk on fire".into())
            } else {
                Ok(())
            }
        });
        let res = run_sharded(
            4,
            vec![counting_factory(0, shut.clone()), failing],
            false,
            |_| served.store(true, Ordering::SeqCst),
        );
        let err = res.expect_err("startup must fail");
        assert!(err.contains("worker 2"), "error must name the worker: {err}");
        assert!(err.contains("service #1"), "error must name the service: {err}");
        assert!(err.contains("disk on fire"), "error must carry the cause: {err}");
        // Every worker — the failing one AND the ones that started fine — must
        // have shut down service #0 (rollback is all-or-nothing).
        let mut shut = shut.lock().unwrap().clone();
        shut.sort();
        assert_eq!(shut, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        assert!(!served.load(Ordering::SeqCst));
    }

    #[test]
    fn factory_panic_fails_startup_deterministically() {
        let panicking = PerWorkerServiceFactory::new(|w: WorkerContext| async move {
            if w.id() == 0 {
                panic!("factory exploded");
            }
            Ok::<(), BoxError>(())
        });
        let err = run_sharded(2, vec![panicking], false, |_| {}).expect_err("must fail");
        assert!(err.contains("worker 0"), "error must name the worker: {err}");
    }

    // ── Builder-level end to end ────────────────────────────────────────────

    /// Sharded serving is what per-worker services live on, and `dev-reload`
    /// deliberately forces the single cached-listener path (see
    /// `docs/features/19-sharded-serving.md` § Limitations (v1)) — so under
    /// that feature `run()` rejects the registration instead of serving. The
    /// rejection is asserted by
    /// `per_worker_service_under_dev_reload_is_a_run_error` below.
    #[cfg(not(feature = "dev-reload"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn builder_per_worker_service_runs_and_stops_via_stop_handle() {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let yaml = format!("server:\n  workers: 2\n  port: {port}\n");
        let config = R2eConfig::from_yaml_str(&yaml).unwrap();
        let started = Arc::new(AtomicUsize::new(0));
        let shut = Arc::new(Mutex::new(Vec::new()));

        let app = AppBuilder::new()
            .override_config(config)
            .load_config::<()>()
            .with_state(())
            .register_routes(
                r2e_core::http::Router::new()
                    .route("/ping", r2e_core::http::routing::get(|| async { "pong" })),
            )
            .per_worker_service({
                let started = started.clone();
                let shut = shut.clone();
                move |w: WorkerContext| {
                    let started = started.clone();
                    let shut = shut.clone();
                    async move {
                        started.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, BoxError>(Counting {
                            shut,
                            worker: w.id(),
                            idx: 0,
                        })
                    }
                }
            })
            .prepare(&addr);
        let stop = app.stop_handle();

        let server = r2e_core::rt::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

        let addr2 = addr.clone();
        r2e_core::rt::spawn_blocking(move || wait_ready(&addr2))
            .await
            .unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 2);

        stop.stop();
        let joined = r2e_core::rt::timeout(Duration::from_secs(10), server).await;
        match joined {
            Ok(Ok(Ok(()))) => {}
            other => panic!("server did not stop cleanly: {other:?}"),
        }
        let mut shut = shut.lock().unwrap().clone();
        shut.sort();
        assert_eq!(shut, vec![(0, 0), (1, 0)]);
    }
}
