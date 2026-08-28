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
use std::sync::Mutex;
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
