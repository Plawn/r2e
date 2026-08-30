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
            let listener = serve_ctx
                .bind_tcp("127.0.0.1:0")
                .expect("bind through the dev listener store");
            *BOUND.lock().unwrap() = Some(listener.local_addr().unwrap());
            let cycle = CYCLE.load(Ordering::SeqCst);
            let shutdown = serve_ctx.shutdown_token();
            serve_ctx.track_named("cycle echo", async move {
                loop {
                    let accepted = r2e_core::rt::select! {
                        _ = shutdown.cancelled() => break,
                        accepted = listener.accept() => accepted,
                    };
                    if let Ok((mut stream, _)) = accepted {
                        let _ = stream.write_all(cycle.to_string().as_bytes()).await;
                    }
                }
            });
        });
        Ok(())
    }
}

/// Ask the served port which cycle answers.
async fn served_cycle(addr: SocketAddr) -> String {
    let mut stream = r2e_core::rt::timeout(
        Duration::from_secs(5),
        r2e_core::rt::TcpStream::connect(addr),
    )
    .await
    .expect("connect timed out")
    .expect("connect");
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await.expect("read");
    buf
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cycle_serve_hook_keeps_the_port_across_hot_patches() {
    let _serial = crate::serial::dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::runtime::dev::mark_hot_reload_loop();

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
        first.run_with_listener(http1).await.map_err(|e| e.to_string())
    });
    r2e_core::rt::sleep(Duration::from_millis(50)).await;
    let addr1 = BOUND.lock().unwrap().expect("cycle 1 bound its port");
    assert_eq!(served_cycle(addr1).await, "1");
    assert_eq!(ONCE_HOOK_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(EACH_CYCLE_HOOK_RUNS.load(Ordering::SeqCst), 1);

    // ── Hot patch: the loop drops the running server and rebuilds ────────
    first_server.abort();
    let _ = first_server.await;

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
        second.run_with_listener(http2).await.map_err(|e| e.to_string())
    });
    r2e_core::rt::sleep(Duration::from_millis(50)).await;

    // Same port, served by the REBUILT cycle; the once-hook did not re-run.
    let addr2 = BOUND.lock().unwrap().expect("cycle 2 bound its port");
    assert_eq!(addr2, addr1, "the dev listener store must hand back the same socket");
    assert_eq!(served_cycle(addr2).await, "2");
    assert_eq!(ONCE_HOOK_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(EACH_CYCLE_HOOK_RUNS.load(Ordering::SeqCst), 2);

    stop.stop();
    let result = r2e_core::rt::timeout(Duration::from_secs(5), second_server)
        .await
        .expect("replacement server did not stop")
        .expect("replacement server task panicked");
    assert!(result.is_ok(), "replacement server failed: {result:?}");
}
