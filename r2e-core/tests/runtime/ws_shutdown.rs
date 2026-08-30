//! Upgraded WebSocket sessions and the shutdown budgets.
//!
//! An upgraded socket is invisible to hyper's graceful drain (the connection
//! counts as finished at upgrade) and axum spawns the session detached, so
//! before #979 a session was watched by neither `drain_timeout` nor
//! `shutdown_grace_period` and simply died with the runtime. Generated `#[ws]`
//! routes now run their session on the tracked lane
//! ([`WsSessions`](r2e_core::builder::WsSessions)):
//!
//! - a session that ignores shutdown is bounded by `shutdown_grace_period` and
//!   named `ws:<Controller>::<method>` in the warning, and `on_stop` still runs;
//! - a session sitting on `WsStream::next` gets a `1001 Going Away` close frame
//!   and finishes **before** `on_stop`;
//! - an app that is never served (`build()` / `build_with_consumers()`, i.e.
//!   `TestApp`) leaves the registry unarmed and sessions run exactly as before.
//!
//! Same current-thread-runtime rule as `shutdown_budget`: the shutdown path's
//! `tracing` events must be emitted on the thread that installed the capturing
//! subscriber.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use r2e_core::prelude::*;
use r2e_core::web::ws::WsStream;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

use crate::shutdown_budget::{capturing, current_thread_rt, Captured};

type ClientStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ── Fixture controller ──────────────────────────────────────────────────────

static STUBBORN_SESSION_STARTED: AtomicBool = AtomicBool::new(false);

/// Ordered trace of "the polite session ended" vs "on_stop ran", so the test
/// can assert the *order* and not merely that both happened.
static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record(event: &'static str) {
    ORDER.lock().unwrap().push(event);
}

#[controller]
struct WsShutdownController {}

#[routes]
impl WsShutdownController {
    /// Never touches the receive side, so nothing tells it to stop: the
    /// grace period is the only thing that ends this session.
    #[ws("/stubborn")]
    async fn stubborn(&self, mut ws: WsStream) {
        STUBBORN_SESSION_STARTED.store(true, Ordering::SeqCst);
        ws.send_text("hello").await.ok();
        r2e_core::rt::sleep(Duration::from_secs(60)).await;
    }

    /// Deliberately slow to react: it waits for the shutdown signal, then
    /// spends ~800ms "reconciling" before saying goodbye — long after a
    /// sharded worker would have left its serve loop. The socket must still be
    /// usable: the session holds the grace period, so it holds the socket.
    #[ws("/slow-goodbye")]
    async fn slow_goodbye(&self, mut ws: WsStream) {
        ws.shutdown_requested().await;
        r2e_core::rt::sleep(Duration::from_millis(800)).await;
        match ws.send_text("bye").await {
            Ok(()) => record("goodbye-sent"),
            Err(_) => record("goodbye-failed"),
        }
    }

    /// The ordinary loop shape. `next()` reports end-of-stream when the app
    /// shuts down (after sending the going-away frame), so this returns on its
    /// own — before `on_stop`, since the session is a tracked handle.
    #[ws("/polite")]
    async fn polite(&self, mut ws: WsStream) {
        while let Some(Ok(msg)) = ws.next().await {
            if let r2e_core::http::ws::Message::Text(t) = msg {
                ws.send_text(t.to_string()).await.ok();
            }
        }
        record("session-ended");
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Boot the fixture controller on a real listener and connect a client to
/// `path`. Returns the client, the app's `StopHandle` and the server's join
/// handle.
async fn serve_and_connect(
    path: &str,
    grace: Duration,
) -> (
    ClientStream,
    r2e_core::StopHandle,
    r2e_core::rt::JobHandle<Result<(), String>>,
) {
    let listener = r2e_core::rt::bind_tcp("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = AppBuilder::new()
        .build_state()
        .await
        .register_controller::<WsShutdownController>()
        .shutdown_grace_period(grace)
        .on_stop(|_state| async {
            record("on-stop");
        })
        .prepare(&addr.to_string());
    let stop = app.stop_handle();
    let server = r2e_core::rt::spawn(async move {
        app.run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
        .await
        .expect("websocket upgrade");
    (client, stop, server)
}

/// The fixture's statics (`ORDER`, `STUBBORN_SESSION_STARTED`) are shared by
/// every test in this module, so they run one at a time.
fn ws_serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn reset() {
    STUBBORN_SESSION_STARTED.store(false, Ordering::SeqCst);
    ORDER.lock().unwrap().clear();
}

fn order() -> Vec<&'static str> {
    ORDER.lock().unwrap().clone()
}

// ── 1. A session that ignores shutdown is bounded, and named ────────────────

#[test]
fn grace_period_bounds_a_stubborn_ws_session_and_names_it() {
    let _serial = ws_serial();
    reset();
    let (captured, subscriber) = capturing();

    let rt = current_thread_rt();
    let elapsed = tracing::subscriber::with_default(subscriber, || {
        rt.block_on(async {
            let (mut client, stop, server) =
                serve_and_connect("/stubborn", Duration::from_millis(300)).await;

            // Wait until the session is actually running before stopping, so
            // the handle is on the tracked lane when the drain begins.
            let greeting = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("no greeting before timeout");
            assert!(matches!(greeting, Some(Ok(ClientMessage::Text(_)))));
            assert!(STUBBORN_SESSION_STARTED.load(Ordering::SeqCst));

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
        "shutdown must not wait for the 60s session: took {elapsed:?}"
    );
    assert_eq!(
        order(),
        vec!["on-stop"],
        "on_stop must run even though the session was abandoned"
    );
    assert_warns(
        &captured,
        "shutdown_grace_period elapsed",
        "the grace-period warning",
    );
    assert_warns(
        &captured,
        "ws:WsShutdownController::stubborn",
        "the ws:<Controller>::<method> label",
    );
}

fn assert_warns(captured: &Captured, needle: &str, what: &str) {
    assert!(
        captured.contains(needle),
        "expected {what} ({needle:?}), got:\n{}",
        captured.dump()
    );
}

// ── 2. A well-behaved session gets 1001 and ends before on_stop ─────────────

#[test]
fn cooperative_ws_session_is_closed_with_going_away_before_on_stop() {
    let _serial = ws_serial();
    reset();
    let (captured, subscriber) = capturing();

    tracing::subscriber::with_default(subscriber, || {
        current_thread_rt().block_on(async {
            let (mut client, stop, server) =
                serve_and_connect("/polite", Duration::from_secs(30)).await;

            // Round-trip once so the session is provably inside its loop.
            client
                .send(ClientMessage::text("ping"))
                .await
                .expect("send ping");
            let echoed = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("no echo before timeout");
            match echoed {
                Some(Ok(ClientMessage::Text(t))) => assert_eq!(t.as_str(), "ping"),
                other => panic!("expected the echo, got {other:?}"),
            }

            stop.stop();

            // The server tells the peer why it went away, rather than dropping
            // the socket on the floor.
            let closed = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("no close frame before timeout");
            match closed {
                Some(Ok(ClientMessage::Close(Some(frame)))) => {
                    assert_eq!(u16::from(frame.code), 1001, "expected 1001 Going Away");
                }
                other => panic!("expected a close frame, got {other:?}"),
            }

            match r2e_core::rt::timeout(Duration::from_secs(15), server).await {
                Ok(Ok(Ok(()))) => {}
                other => panic!("server did not stop cleanly: {other:?}"),
            }
        })
    });

    assert_eq!(
        order(),
        vec!["session-ended", "on-stop"],
        "the session must be joined BEFORE on_stop, which is the whole point \
         of putting it on the tracked lane"
    );
    assert!(
        !captured.contains("shutdown_grace_period elapsed"),
        "a cooperative session must not consume the grace period, got:\n{}",
        captured.dump()
    );
}

// ── 3. No serve, no tracking — and the session still works ──────────────────

#[test]
fn unserved_app_leaves_sessions_untracked_and_working() {
    let _serial = ws_serial();
    reset();

    current_thread_rt().block_on(async {
        // The `TestApp` / `build_with_consumers` shape: a Router, never run().
        let app = AppBuilder::new()
            .build_state()
            .await
            .register_controller::<WsShutdownController>();

        // Nothing armed the registry, so `run_session` runs the body inline in
        // axum's detached task — exactly the pre-#979 behaviour.
        let sessions = app
            .bean_context()
            .try_get::<r2e_core::builder::WsSessions>()
            .expect("WsSessions is provided by AppBuilder::new");
        assert!(!sessions.is_armed());

        let router = app.build_with_consumers().await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        r2e_core::rt::spawn(async move {
            r2e_core::http::serve(listener, router).await.unwrap();
        });

        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/polite"))
            .await
            .expect("websocket upgrade");
        client
            .send(ClientMessage::text("still here"))
            .await
            .unwrap();
        let echoed = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("no echo before timeout");
        match echoed {
            Some(Ok(ClientMessage::Text(t))) => assert_eq!(t.as_str(), "still here"),
            other => panic!("expected the echo, got {other:?}"),
        }
    });
}

// ── 4. Sharded serving (SO_REUSEPORT): same guarantees ──────────────────────
//
// Under `server.workers` the upgraded socket was accepted by a **worker**
// runtime (a `current_thread` runtime on its own thread) while the session
// itself runs on the control plane. The worker's I/O driver must therefore
// still be alive when the session is joined, or the going-away frame would be
// written into a dead reactor. See `builder/ws_sessions.rs` § "Sharded
// serving" for the invariant these tests pin down.

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

    /// A `current_thread` control plane on purpose: `serve_sharded` blocks on
    /// a *blocking* thread, so nothing here needs worker threads — and with a
    /// single thread every shutdown-phase `tracing` event (including the ones
    /// `drain_tracked_handles` emits from its `JoinSet`) lands on the thread
    /// holding the capturing subscriber. `run()` logs one warning about the
    /// non-multi-thread control plane, which is fine in a test.
    fn control_plane_rt() -> r2e_core::rt::Runtime {
        current_thread_rt()
    }

    async fn connect_retry(url: &str) -> ClientStream {
        for _ in 0..200 {
            if let Ok((client, _)) = tokio_tungstenite::connect_async(url).await {
                return client;
            }
            r2e_core::rt::sleep(Duration::from_millis(25)).await;
        }
        panic!("sharded server never accepted a websocket connection on {url}");
    }

    /// Stops the app even when the client task unwinds, so a failing assertion
    /// surfaces as that assertion instead of a hung `run()`.
    struct StopOnDrop(r2e_core::StopHandle);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.stop();
        }
    }

    /// Build the fixture app on `port` with `workers: 2`.
    async fn sharded_app(
        port: u16,
        grace: Duration,
    ) -> r2e_core::builder::PreparedApp<impl Clone + Send + Sync + 'static + BeanLookup> {
        let yaml = format!("server:\n  workers: 2\n  port: {port}\n");
        let config = R2eConfig::from_yaml_str(&yaml).unwrap();
        let builder = AppBuilder::new()
            .override_config(config)
            .load_config::<()>();
        builder
            .build_state()
            .await
            .register_controller::<WsShutdownController>()
            .shutdown_grace_period(grace)
            .on_stop(|_state| async {
                record("on-stop");
            })
            .prepare(&format!("127.0.0.1:{port}"))
    }

    /// (c) Sanity: a session on a sharded worker exchanges messages normally.
    #[test]
    fn sharded_ws_session_echoes_while_served() {
        let _serial = ws_serial();
        reset();

        control_plane_rt().block_on(async {
            let port = free_port();
            let app = sharded_app(port, Duration::from_secs(30)).await;
            let stop = app.stop_handle();

            let client = r2e_core::rt::spawn(async move {
                let _stop_on_drop = StopOnDrop(stop);
                let mut client = connect_retry(&format!("ws://127.0.0.1:{port}/polite")).await;
                for i in 0..3 {
                    let payload = format!("msg-{i}");
                    client
                        .send(ClientMessage::text(payload.clone()))
                        .await
                        .expect("send");
                    let echoed = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                        .await
                        .expect("no echo before timeout");
                    match echoed {
                        Some(Ok(ClientMessage::Text(t))) => assert_eq!(t.as_str(), payload),
                        other => panic!("expected the echo, got {other:?}"),
                    }
                }
            });

            app.run().await.expect("sharded server failed");
            client.await.expect("client task panicked");
        });

        assert_eq!(
            order(),
            vec!["session-ended", "on-stop"],
            "the sharded session must end on its own, before on_stop"
        );
    }

    /// (a) A cooperative session on a worker still gets its `1001 Going Away`
    /// frame — i.e. the socket is still writable when the session is joined.
    #[test]
    fn sharded_cooperative_ws_session_is_closed_with_going_away_before_on_stop() {
        let _serial = ws_serial();
        reset();
        let (captured, subscriber) = capturing();

        tracing::subscriber::with_default(subscriber, || {
            control_plane_rt().block_on(async {
                let port = free_port();
                let app = sharded_app(port, Duration::from_secs(30)).await;
                let stop = app.stop_handle();

                let client = r2e_core::rt::spawn(async move {
                    let _stop_on_drop = StopOnDrop(stop);
                    let mut client = connect_retry(&format!("ws://127.0.0.1:{port}/polite")).await;
                    client
                        .send(ClientMessage::text("ping"))
                        .await
                        .expect("send ping");
                    let echoed = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                        .await
                        .expect("no echo before timeout");
                    match echoed {
                        Some(Ok(ClientMessage::Text(t))) => assert_eq!(t.as_str(), "ping"),
                        other => panic!("expected the echo, got {other:?}"),
                    }

                    _stop_on_drop.0.stop();

                    let closed = r2e_core::rt::timeout(Duration::from_secs(10), client.next())
                        .await
                        .expect("no close frame before timeout");
                    match closed {
                        Some(Ok(ClientMessage::Close(Some(frame)))) => {
                            assert_eq!(u16::from(frame.code), 1001, "expected 1001 Going Away");
                        }
                        other => panic!("expected a close frame, got {other:?}"),
                    }
                });

                app.run().await.expect("sharded server failed");
                client.await.expect("client task panicked");
            })
        });

        assert_eq!(
            order(),
            vec!["session-ended", "on-stop"],
            "the sharded session must be joined BEFORE on_stop"
        );
        assert!(
            !captured.contains("shutdown_grace_period elapsed"),
            "a cooperative sharded session must not consume the grace period, got:\n{}",
            captured.dump()
        );
    }

    /// (b) A stubborn session on a worker is bounded by the grace period and
    /// named, exactly like on the single-listener path.
    #[test]
    fn sharded_grace_period_bounds_a_stubborn_ws_session_and_names_it() {
        let _serial = ws_serial();
        reset();
        let (captured, subscriber) = capturing();

        let elapsed = tracing::subscriber::with_default(subscriber, || {
            control_plane_rt().block_on(async {
                let port = free_port();
                let app = sharded_app(port, Duration::from_millis(300)).await;
                let stop = app.stop_handle();

                let stopped_at = Arc::new(Mutex::new(None::<Instant>));
                let stopped_at_client = stopped_at.clone();
                let client = r2e_core::rt::spawn(async move {
                    let _stop_on_drop = StopOnDrop(stop);
                    let mut client =
                        connect_retry(&format!("ws://127.0.0.1:{port}/stubborn")).await;
                    let greeting = r2e_core::rt::timeout(Duration::from_secs(5), client.next())
                        .await
                        .expect("no greeting before timeout");
                    assert!(matches!(greeting, Some(Ok(ClientMessage::Text(_)))));
                    assert!(STUBBORN_SESSION_STARTED.load(Ordering::SeqCst));
                    *stopped_at_client.lock().unwrap() = Some(Instant::now());
                });

                app.run().await.expect("sharded server failed");
                client.await.expect("client task panicked");
                let stopped_at = stopped_at.lock().unwrap().expect("client never stopped");
                stopped_at.elapsed()
            })
        });

        assert!(
            elapsed < Duration::from_secs(10),
            "sharded shutdown must not wait for the 60s session: took {elapsed:?}"
        );
        assert_eq!(
            order(),
            vec!["on-stop"],
            "on_stop must run even though the sharded session was abandoned"
        );
        assert_warns(
            &captured,
            "shutdown_grace_period elapsed",
            "the grace-period warning",
        );
        assert_warns(
            &captured,
            "ws:WsShutdownController::stubborn",
            "the ws:<Controller>::<method> label",
        );
    }

    /// The hazard this module exists to pin down: the session runs on the
    /// control plane, but its socket was accepted by a **worker** runtime whose
    /// I/O driver dies with it. Before the worker-parking handshake this test
    /// failed with `ResetWithoutClosingHandshake` — the worker had dropped its
    /// runtime as soon as its serve loop ended, and the session's socket died
    /// under it 800ms into a 30s grace period.
    #[test]
    fn sharded_slow_session_still_owns_a_writable_socket_during_the_grace_period() {
        let _serial = ws_serial();
        reset();

        control_plane_rt().block_on(async {
            let port = free_port();
            let app = sharded_app(port, Duration::from_secs(30)).await;
            let stop = app.stop_handle();

            let client = r2e_core::rt::spawn(async move {
                let _stop_on_drop = StopOnDrop(stop);
                let mut client =
                    connect_retry(&format!("ws://127.0.0.1:{port}/slow-goodbye")).await;
                _stop_on_drop.0.stop();
                let goodbye = r2e_core::rt::timeout(Duration::from_secs(10), client.next())
                    .await
                    .expect("no goodbye before timeout");
                match goodbye {
                    Some(Ok(ClientMessage::Text(t))) => assert_eq!(t.as_str(), "bye"),
                    other => panic!("expected the late goodbye, got {other:?}"),
                }
            });

            app.run().await.expect("sharded server failed");
            client.await.expect("client task panicked");
        });

        assert_eq!(
            order(),
            vec!["goodbye-sent", "on-stop"],
            "a slow session must still own a writable socket, and be joined \
             before on_stop"
        );
    }
}
