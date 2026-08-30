//! Serve hooks across hot-patch cycles (task #997).
//!
//! A hot patch drops the previous cycle's `run()` future: its cancel-on-exit
//! guard fires and every task that cycle's serve hooks tracked stops. The
//! rebuilt cycle then skips the startup lifecycle — including plain
//! `on_serve` hooks — so a transport that serves its own port from such a
//! hook (the separate-port gRPC server) went silent after the first patch.
//!
//! `on_serve_each_cycle` + `ServeContext::bind_tcp` is the fix: the hook
//! re-runs on every cycle and the listener comes from the dev listener store,
//! so the port stays open — and stays the same port — across cycles. This
//! drives two serving cycles by hand, the way `r2e::launch!` does, against a
//! plugin standing in for the gRPC transport.
use crate::serial::CommitCycle;
use r2e_core::plugin::{PluginBuildContext, PluginBuildError};
use r2e_core::rt::io::{AsyncReadExt, AsyncWriteExt};
use r2e_core::{AppBuilder, Plugin};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Cycle counter the served task answers with — bumped per cycle so the
/// client can tell WHICH cycle's server it reached.
static CYCLE: AtomicU32 = AtomicU32::new(0);
static ONCE_HOOK_RUNS: AtomicU32 = AtomicU32::new(0);
static EACH_CYCLE_HOOK_RUNS: AtomicU32 = AtomicU32::new(0);
/// Accept loops that observed their stop signal and exited.
static STOPPED_LOOPS: AtomicU32 = AtomicU32::new(0);
/// The port the each-cycle hook bound (`:0` → OS-assigned); the second cycle
/// must land on the same one.
static BOUND: Mutex<Option<SocketAddr>> = Mutex::new(None);

/// Stand-in for a separate-port transport: one hook that must run once per
/// process, one that must re-serve its port on every cycle.
struct PortPlugin;

impl Plugin for PortPlugin {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        ctx.on_serve(|_serve_ctx| {
            ONCE_HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
        });
        ctx.on_serve_each_cycle(|serve_ctx| {
            EACH_CYCLE_HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
            let bind = serve_ctx.bind_tcp("test-port", "127.0.0.1:0");
            let cycle = CYCLE.load(Ordering::SeqCst);
            let shutdown = serve_ctx.shutdown_token();
            serve_ctx.track_named("cycle echo", async move {
                let bound = bind.await.expect("bind through the dev listener store");
                *BOUND.lock().unwrap() = Some(bound.listener.local_addr().unwrap());
                let stop = bound.stop_signal(shutdown);
                let listener = bound.listener;
                r2e_core::rt::pin!(stop);
                loop {
                    let accepted = r2e_core::rt::select! {
                        _ = &mut stop => break,
                        accepted = listener.accept() => accepted,
                    };
                    if let Ok((mut stream, _)) = accepted {
                        let _ = stream.write_all(cycle.to_string().as_bytes()).await;
                    }
                }
                STOPPED_LOOPS.fetch_add(1, Ordering::SeqCst);
            });
        });
        Ok(())
    }
}

/// Ask the served port which cycle answers. Bounded: a port nobody serves
/// accepts (the socket is bound) but never answers.
async fn served_cycle(addr: SocketAddr) -> String {
    r2e_core::rt::timeout(Duration::from_secs(5), async {
        let mut stream = r2e_core::rt::TcpStream::connect(addr)
            .await
            .expect("connect");
        let mut buf = String::new();
        stream.read_to_string(&mut buf).await.expect("read");
        buf
    })
    .await
    .expect("the served port did not answer within 5s")
}

/// Wait until the each-cycle hook of the given run bound its port.
async fn wait_bound(expected_runs: u32) -> SocketAddr {
    r2e_core::rt::timeout(Duration::from_secs(5), async {
        loop {
            if EACH_CYCLE_HOOK_RUNS.load(Ordering::SeqCst) >= expected_runs {
                if let Some(addr) = *BOUND.lock().unwrap() {
                    return addr;
                }
            }
            r2e_core::rt::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the each-cycle hook did not bind its port within 5s")
}

async fn wait_stopped(expected: u32) {
    r2e_core::rt::timeout(Duration::from_secs(5), async {
        while STOPPED_LOOPS.load(Ordering::SeqCst) < expected {
            r2e_core::rt::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the previous cycle's accept loop did not stop within 5s");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cycle_serve_hook_keeps_the_port_across_hot_patches() {
    let _serial = crate::serial::dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::runtime::dev::mark_hot_reload_loop();
    ONCE_HOOK_RUNS.store(0, Ordering::SeqCst);
    EACH_CYCLE_HOOK_RUNS.store(0, Ordering::SeqCst);
    STOPPED_LOOPS.store(0, Ordering::SeqCst);
    *BOUND.lock().unwrap() = None;

    // ── Cycle 1: cold start — both hooks run, the port opens ─────────────
    CYCLE.store(1, Ordering::SeqCst);
    let first = AppBuilder::new()
        .plugin(PortPlugin)
        .build_state()
        .await
        .commit_cycle()
        .prepare("127.0.0.1:0");
    let http1 = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
    let first_server = r2e_core::rt::spawn(async move {
        first
            .run_with_listener(http1)
            .await
            .map_err(|e| e.to_string())
    });
    let addr1 = wait_bound(1).await;
    assert_eq!(served_cycle(addr1).await, "1");
    assert_eq!(ONCE_HOOK_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(EACH_CYCLE_HOOK_RUNS.load(Ordering::SeqCst), 1);

    // ── Hot patch: the loop drops the running server and rebuilds ────────
    first_server.abort();
    let _ = first_server.await;
    // Dropping `run()` cancels the cycle token; the tracked loop exits.
    wait_stopped(1).await;

    CYCLE.store(2, Ordering::SeqCst);
    let second = AppBuilder::new()
        .plugin(PortPlugin)
        .build_state()
        .await
        .commit_cycle()
        .prepare("127.0.0.1:0");
    let stop = second.stop_handle();
    let http2 = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
    let second_server = r2e_core::rt::spawn(async move {
        second
            .run_with_listener(http2)
            .await
            .map_err(|e| e.to_string())
    });

    // Same port, served by the REBUILT cycle; the once-hook did not re-run.
    let addr2 = wait_bound(2).await;
    assert_eq!(
        addr2, addr1,
        "the dev listener store must hand back the same socket"
    );
    assert_eq!(served_cycle(addr2).await, "2");
    assert_eq!(ONCE_HOOK_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(EACH_CYCLE_HOOK_RUNS.load(Ordering::SeqCst), 2);

    stop.stop();
    let result = r2e_core::rt::timeout(Duration::from_secs(5), second_server)
        .await
        .expect("replacement server did not stop")
        .expect("replacement server task panicked");
    assert!(result.is_ok(), "replacement server failed: {result:?}");
    wait_stopped(2).await;
}

/// The handover itself: a server whose cycle was NOT cancelled (it leaked —
/// the tracked task is detached, never joined) must still stop accepting the
/// moment the next cycle takes the socket, so it cannot answer with stale
/// routes.
#[tokio::test(flavor = "multi_thread")]
async fn taking_the_socket_again_stops_the_previous_holder() {
    let _serial = crate::serial::dev_serial();
    r2e_core::runtime::dev::mark_hot_reload_loop();
    // `ServeContext::bind_tcp` is the store call below under the loop; the
    // store is what the handover lives in.
    let first =
        r2e_core::runtime::dev::get_or_bind_listener("handover-test", "127.0.0.1:0").unwrap();
    let addr = first.listener.local_addr().unwrap();
    let never = r2e_core::rt::CancelToken::new();
    let first_stop = first.stop_signal(never.clone());
    let (tx, rx) = r2e_core::rt::sync::oneshot::channel();
    let old_loop = r2e_core::rt::spawn(async move {
        let listener = first.listener;
        r2e_core::rt::pin!(first_stop);
        let mut served = 0u32;
        loop {
            let accepted = r2e_core::rt::select! {
                _ = &mut first_stop => break,
                accepted = listener.accept() => accepted,
            };
            if let Ok((mut stream, _)) = accepted {
                let _ = stream.write_all(b"old").await;
                served += 1;
            }
        }
        let _ = tx.send(served);
    });
    assert_eq!(served_cycle(addr).await, "old");

    // The next cycle takes the socket: the old loop stops without anyone
    // cancelling its shutdown token.
    let second =
        r2e_core::runtime::dev::get_or_bind_listener("handover-test", "127.0.0.1:0").unwrap();
    assert_eq!(second.listener.local_addr().unwrap(), addr);
    let served_by_old = r2e_core::rt::timeout(Duration::from_secs(5), rx)
        .await
        .expect("old holder did not stop after the handover")
        .unwrap();
    assert_eq!(served_by_old, 1);
    let _ = old_loop.await;

    // Connections queued now are answered by the new holder only.
    let second_stop = second.stop_signal(never.clone());
    let new_loop = r2e_core::rt::spawn(async move {
        let listener = second.listener;
        r2e_core::rt::pin!(second_stop);
        loop {
            let accepted = r2e_core::rt::select! {
                _ = &mut second_stop => break,
                accepted = listener.accept() => accepted,
            };
            if let Ok((mut stream, _)) = accepted {
                let _ = stream.write_all(b"new").await;
            }
        }
    });
    assert_eq!(served_cycle(addr).await, "new");
    never.cancel();
    let _ = r2e_core::rt::timeout(Duration::from_secs(5), new_loop).await;
}
