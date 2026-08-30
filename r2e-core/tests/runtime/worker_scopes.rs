//! Worker scopes (task #990): `WorkerInfo`, `WorkerLocal<T>`, `WorkerSet`
//! lifecycle, `Mailboxes<M>` routing/crossing accounting, `WorkerHarness`
//! drain order, and the ingress affinity helpers — all deterministic, no HTTP.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::rt::sync::oneshot;
use r2e_core::runtime::worker::{PerWorkerServiceFactory, WorkerContext, WorkerService};
use r2e_core::{
    reuseport_supported, reuseport_tcp, reuseport_udp, MailboxError, Mailboxes, WorkerHarness,
    WorkerInfo, WorkerLocal, WorkerRole, WorkerSet, WorkerState,
};

// ── WorkerInfo ──────────────────────────────────────────────────────────

#[test]
fn worker_info_defaults_to_control_plane_off_worker() {
    assert!(WorkerInfo::current().is_none());
    let cp = WorkerInfo::current_or_control_plane();
    assert_eq!(cp.role(), WorkerRole::ControlPlane);
    assert!(!cp.is_data_plane());
    assert_eq!(cp.to_string(), "control-plane");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_info_is_installed_on_each_harness_worker() {
    let h = WorkerHarness::start(3, vec![]).await.unwrap();
    let seen = h
        .run_on_all(|ctx| async move {
            let info = WorkerInfo::current().expect("installed on worker");
            (
                ctx.id(),
                info.id(),
                info.workers(),
                info.role(),
                info.to_string(),
            )
        })
        .await;
    for (i, row) in seen.iter().enumerate() {
        assert_eq!(
            *row,
            (i, i, 3, WorkerRole::DataPlane, format!("worker {i}/3"))
        );
    }
    h.shutdown().await;
}

// ── WorkerLocal ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_local_is_built_exactly_once_per_worker_and_dropped_on_shutdown() {
    struct Tracked(Arc<AtomicUsize>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let builds = Arc::new(AtomicUsize::new(0));
    let local: WorkerLocal<Rc<Tracked>> = WorkerLocal::new({
        let drops = drops.clone();
        let builds = builds.clone();
        move |_ctx: WorkerContext| {
            let drops = drops.clone();
            builds.fetch_add(1, Ordering::SeqCst);
            async move { Ok(Rc::new(Tracked(drops))) }
        }
    });
    assert_eq!(local.instances(), 0);
    assert!(!local.is_installed());

    let h = WorkerHarness::start(4, vec![local.clone().into_factory()])
        .await
        .unwrap();
    assert_eq!(local.instances(), 4);
    assert_eq!(local.built(), 4);
    assert_eq!(builds.load(Ordering::SeqCst), 4);
    // Off-worker: not installed here, `try_with` is None.
    assert!(!local.is_installed());
    assert!(local.try_with(|_| ()).is_none());

    // On-worker: installed, and the Rc is distinct per worker.
    let ptrs = h
        .run_on_all({
            let local = local.clone();
            move |_| {
                let local = local.clone();
                async move {
                    assert!(local.is_installed());
                    local.with(|t| Rc::as_ptr(t) as usize)
                }
            }
        })
        .await;
    let mut uniq = ptrs.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), 4, "one distinct instance per worker: {ptrs:?}");

    h.shutdown().await;
    assert_eq!(local.instances(), 0);
    assert_eq!(local.dropped(), 4);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        4,
        "dropped on the worker at shutdown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_local_with_panics_off_worker_naming_the_caller() {
    let local: WorkerLocal<Cell<u64>> = WorkerLocal::new(|_| async { Ok(Cell::new(7)) });
    let h = WorkerHarness::start(1, vec![local.clone().into_factory()])
        .await
        .unwrap();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| local.with(|c| c.get())))
        .expect_err("reading a worker-local off its worker must panic");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        msg.contains("control-plane"),
        "panic names the caller: {msg}"
    );
    assert!(msg.contains("Cell<u64>"), "panic names the type: {msg}");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_local_state_is_mutable_and_stays_on_its_worker() {
    let local: WorkerLocal<RefCell<Vec<usize>>> =
        WorkerLocal::new(|_| async { Ok(RefCell::new(Vec::new())) });
    let h = WorkerHarness::start(2, vec![local.clone().into_factory()])
        .await
        .unwrap();
    for _ in 0..3 {
        h.run_on(1, {
            let local = local.clone();
            move |ctx| async move { local.with(|v| v.borrow_mut().push(ctx.id())) }
        })
        .await;
    }
    let lens = h
        .run_on_all({
            let local = local.clone();
            move |_| {
                let local = local.clone();
                async move { local.with(|v| v.borrow().clone()) }
            }
        })
        .await;
    assert_eq!(lens, vec![vec![], vec![1, 1, 1]]);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_local_factory_error_fails_startup_and_unwinds() {
    let local: WorkerLocal<()> = WorkerLocal::new(|ctx: WorkerContext| async move {
        if ctx.id() == 1 {
            Err("boom".into())
        } else {
            Ok(())
        }
    });
    let err = WorkerHarness::start(3, vec![local.clone().into_factory()])
        .await
        .expect_err("worker 1 fails");
    assert!(err.contains("worker 1"), "{err}");
    assert!(err.contains("boom"), "{err}");
    assert_eq!(local.instances(), 0, "started instances unwound");
}

// ── WorkerSet lifecycle ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_set_tracks_lifecycle_through_harness() {
    let set = WorkerSet::new();
    assert_eq!(set.workers(), 0);
    let h = WorkerHarness::start(2, vec![]).await.unwrap();
    let set = h.worker_set().clone();
    assert_eq!(set.workers(), 2);
    assert!(set.all_serving());
    assert!(!set.any_failed());
    assert_eq!(set.states(), vec![WorkerState::Serving; 2]);
    h.shutdown().await;
    assert_eq!(set.states(), vec![WorkerState::Exited; 2]);
    assert!(set.all_in(WorkerState::Exited));
    for snap in set.snapshot() {
        assert_eq!(snap.state, WorkerState::Exited);
        assert!(snap.error.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_set_records_failure_with_message() {
    let boom = PerWorkerServiceFactory::new(|ctx: WorkerContext| async move {
        if ctx.id() == 0 {
            Err::<(), _>("no socket".into())
        } else {
            Ok(())
        }
    });
    let err = WorkerHarness::start(2, vec![boom])
        .await
        .expect_err("fails");
    assert!(err.contains("no socket"), "{err}");
}

#[tokio::test]
async fn worker_set_wait_until_wakes_on_state_change() {
    let set = WorkerSet::new();
    set.configure(1);
    let slot = set.slot(0).unwrap();
    assert_eq!(slot.state(), WorkerState::Unstarted);
    let waiter = {
        let set = set.clone();
        r2e_core::rt::spawn(async move { set.wait_all_serving().await })
    };
    r2e_core::rt::sleep(Duration::from_millis(20)).await;
    slot.set_state(WorkerState::Serving);
    r2e_core::rt::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("waiter woke up")
        .unwrap();
    slot.fail("late failure");
    assert!(set.any_failed());
    assert_eq!(set.first_error(), Some((0, "late failure".to_string())));
    assert_eq!(slot.snapshot().state, WorkerState::Failed);
}

#[test]
fn worker_state_names_are_stable() {
    let names: Vec<&str> = WorkerState::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        [
            "unstarted",
            "starting",
            "ready",
            "serving",
            "draining",
            "services_down",
            "parked",
            "exited",
            "failed"
        ]
    );
    for s in WorkerState::ALL {
        assert_eq!(s.to_string(), s.as_str());
    }
}

// ── Mailboxes ───────────────────────────────────────────────────────────

enum Cmd {
    Incr,
    Read(oneshot::Sender<(usize, u64)>),
}

fn counter_service(mail: Mailboxes<Cmd>) -> PerWorkerServiceFactory {
    PerWorkerServiceFactory::new(move |worker: WorkerContext| {
        let mail = mail.clone();
        async move {
            let mut inbox = mail.attach(&worker)?;
            let count = Rc::new(Cell::new(0u64));
            let id = worker.id();
            worker.spawn_local(async move {
                while let Some(cmd) = inbox.recv().await {
                    match cmd {
                        Cmd::Incr => count.set(count.get() + 1),
                        Cmd::Read(reply) => {
                            let _ = reply.send((id, count.get()));
                        }
                    }
                }
            });
            Ok(())
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mailboxes_route_to_the_intended_worker_and_count_remote_crossings() {
    // The harness owns the live set; the mailboxes report into it.
    let h = WorkerHarness::start(3, vec![]).await.unwrap();
    let mail = Mailboxes::new(h.worker_set().clone(), 16);
    // Attach from inside each worker (what a factory would do).
    h.run_on_all({
        let mail = mail.clone();
        move |worker| {
            let mail = mail.clone();
            async move {
                let mut inbox = mail.attach(&worker).unwrap();
                let count = Rc::new(Cell::new(0u64));
                let id = worker.id();
                worker.spawn_local(async move {
                    while let Some(cmd) = inbox.recv().await {
                        match cmd {
                            Cmd::Incr => count.set(count.get() + 1),
                            Cmd::Read(reply) => {
                                let _ = reply.send((id, count.get()));
                            }
                        }
                    }
                });
            }
        }
    })
    .await;
    assert_eq!(mail.attached(), 3);

    // Control plane → worker 2: remote crossings.
    for _ in 0..5 {
        mail.send_to(2, Cmd::Incr).await.unwrap();
    }
    let (id, n) = mail.ask(2, Cmd::Read).await.unwrap();
    assert_eq!((id, n), (2, 5));
    let (id, n) = mail.ask(0, Cmd::Read).await.unwrap();
    assert_eq!((id, n), (0, 0));

    // Worker 1 → itself: local crossing.
    h.run_on(1, {
        let mail = mail.clone();
        move |_| async move {
            mail.send_to(1, Cmd::Incr).await.unwrap();
        }
    })
    .await;
    // Worker 1 → worker 0: remote crossing attributed to worker 0.
    h.run_on(1, {
        let mail = mail.clone();
        move |_| async move {
            mail.send_to(0, Cmd::Incr).await.unwrap();
        }
    })
    .await;
    let all = mail.ask_all(Cmd::Read).await;
    let all: Vec<_> = all.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(all, vec![(0, 1), (1, 1), (2, 5)]);

    let snap = h.worker_set().snapshot();
    // worker 2: 5 incr + 1 read + 1 ask_all read, all remote.
    assert_eq!(snap[2].remote_crossings, 7);
    assert_eq!(snap[2].local_crossings, 0);
    // worker 1: 1 local incr; reads from control plane are remote.
    assert_eq!(snap[1].local_crossings, 1);
    assert_eq!(snap[1].remote_crossings, 1);
    // worker 0: read + incr-from-worker-1 + ask_all read = 3 remote.
    assert_eq!(snap[0].remote_crossings, 3);
    assert_eq!(snap[0].local_crossings, 0);
    // Every message was consumed.
    for s in &snap {
        assert_eq!(s.mailbox_depth, 0, "{s:?}");
        assert_eq!(s.mailbox_sends, s.local_crossings + s.remote_crossings);
    }
    h.shutdown().await;
    // After shutdown the receivers are gone: sends fail with Closed.
    assert!(matches!(
        mail.send_to(0, Cmd::Incr).await,
        Err(MailboxError::Closed(0))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mailboxes_report_missing_workers_and_full_boxes() {
    let h = WorkerHarness::start(2, vec![]).await.unwrap();
    let mail: Mailboxes<Cmd> = Mailboxes::new(h.worker_set().clone(), 1);
    assert!(matches!(
        mail.send_to(0, Cmd::Incr).await,
        Err(MailboxError::NotAttached(0))
    ));
    assert!(matches!(
        mail.send_to(9, Cmd::Incr).await,
        Err(MailboxError::NoSuchWorker(9))
    ));
    // Attach on worker 0 but never drain it: the second try_send is Full.
    let keep: Arc<Mutex<Option<r2e_core::Mailbox<Cmd>>>> = Arc::new(Mutex::new(None));
    h.run_on(0, {
        let mail = mail.clone();
        let keep = keep.clone();
        move |worker| async move {
            let inbox = mail.attach(&worker).unwrap();
            // Park the receiver on the worker (never polled).
            let keep2 = keep.clone();
            worker.spawn_local(async move {
                let _inbox = inbox;
                let _ = keep2;
                std::future::pending::<()>().await;
            });
        }
    })
    .await;
    assert!(mail.try_send_to(0, Cmd::Incr).is_ok());
    let Err((e, _)) = mail.try_send_to(0, Cmd::Incr) else {
        panic!("second send must be Full");
    };
    assert_eq!(e, MailboxError::Full(0));
    assert_eq!(h.worker_set().slot(0).unwrap().snapshot().mailbox_depth, 1);
    h.shutdown().await;
}

// ── Harness drain order ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harness_shuts_services_down_in_reverse_order_on_the_worker() {
    struct Svc {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        thread: std::thread::ThreadId,
    }
    impl WorkerService for Svc {
        fn shutdown(self: Box<Self>) -> r2e_core::runtime::worker::LocalBoxFuture<'static, ()> {
            assert_eq!(
                std::thread::current().id(),
                self.thread,
                "shutdown on the worker"
            );
            Box::pin(async move {
                self.log.lock().unwrap().push(format!(
                    "{}:{}",
                    WorkerInfo::current().unwrap().id(),
                    self.name
                ));
            })
        }
    }
    let log = Arc::new(Mutex::new(Vec::new()));
    let mk = |name: &'static str, log: Arc<Mutex<Vec<String>>>| {
        PerWorkerServiceFactory::new(move |_ctx: WorkerContext| {
            let log = log.clone();
            async move {
                Ok(Svc {
                    name,
                    log,
                    thread: std::thread::current().id(),
                })
            }
        })
    };
    let h = WorkerHarness::start(2, vec![mk("a", log.clone()), mk("b", log.clone())])
        .await
        .unwrap();
    h.shutdown().await;
    let mut got = log.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, ["0:a", "0:b", "1:a", "1:b"]);
    let raw = log.lock().unwrap().clone();
    for w in 0..2 {
        let a = raw.iter().position(|s| *s == format!("{w}:a")).unwrap();
        let b = raw.iter().position(|s| *s == format!("{w}:b")).unwrap();
        assert!(
            b < a,
            "b (started last) shuts down first on worker {w}: {raw:?}"
        );
    }
}

// ── Ingress ─────────────────────────────────────────────────────────────

#[test]
fn reuseport_helpers_follow_the_platform_contract() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    if reuseport_supported() {
        let a = reuseport_tcp(addr).unwrap();
        let bound = a.local_addr().unwrap();
        let b = reuseport_tcp(bound).expect("second listener shares the port");
        assert_eq!(b.local_addr().unwrap(), bound);
        let u = reuseport_udp(addr).unwrap();
        let ubound = u.local_addr().unwrap();
        let u2 = reuseport_udp(ubound).expect("second UDP socket shares the port");
        assert_eq!(u2.local_addr().unwrap(), ubound);
    } else {
        let e = reuseport_tcp(addr).unwrap_err();
        assert!(matches!(
            e,
            r2e_core::AffinityError::Unsupported { transport: "tcp" }
        ));
        let e = reuseport_udp(addr).unwrap_err();
        assert!(matches!(
            e,
            r2e_core::AffinityError::Unsupported { transport: "udp" }
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopt_udp_registers_the_socket_with_the_worker_runtime() {
    if !reuseport_supported() {
        return;
    }
    let h = WorkerHarness::start(1, vec![]).await.unwrap();
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let sock = reuseport_udp(addr).unwrap();
    let bound = sock.local_addr().unwrap();
    let echoed = h
        .run_on(0, move |worker| async move {
            let sock = worker.adopt_udp(sock).unwrap();
            let client = r2e_core::rt::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            client.send_to(b"ping", bound).await.unwrap();
            let mut buf = [0u8; 8];
            let (n, _) = sock.recv_from(&mut buf).await.unwrap();
            buf[..n].to_vec()
        })
        .await;
    assert_eq!(echoed, b"ping");
    h.shutdown().await;
}

#[allow(dead_code)]
fn _counter_service_is_a_valid_factory(mail: Mailboxes<Cmd>) -> PerWorkerServiceFactory {
    counter_service(mail)
}
