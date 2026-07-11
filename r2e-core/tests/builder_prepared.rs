//! Serve lifecycle: programmatic stop ([`StopHandle`]) and the graceful
//! shutdown sequence of [`PreparedApp::run`] — drain hooks awaited before the
//! listener stops accepting, in-flight requests completing, `on_stop` after
//! the drain.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::builder::AppBuilder;
use r2e_core::http::routing::get;
use r2e_core::StopHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Default)]
struct EventLog(Arc<Mutex<Vec<&'static str>>>);

impl EventLog {
    fn push(&self, event: &'static str) {
        self.0.lock().unwrap().push(event);
    }

    fn entries(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn stop_handle_stops_run_gracefully() {
    let app = AppBuilder::new().build_state().await;
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    assert!(!stop.is_stopped());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    stop.stop();
    assert!(stop.is_stopped());

    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s after StopHandle::stop()")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");
}

#[tokio::test]
async fn with_stop_handle_wires_a_user_created_handle() {
    let stop = StopHandle::new();
    let app = AppBuilder::new()
        .with_stop_handle(stop.clone())
        .build_state()
        .await;
    let prepared = app.prepare("127.0.0.1:0");
    // The prepared app hands back the same handle.
    assert!(!prepared.stop_handle().is_stopped());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    // Stopping via the ORIGINAL handle (created before the builder saw it)
    // terminates the server.
    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok());
}

#[tokio::test]
async fn drain_sequence_finishes_in_flight_requests_and_orders_hooks() {
    let log = EventLog::default();
    let log_for_drain = log.clone();
    let log_for_stop = log.clone();
    let log_for_handler = log.clone();

    let slow_route = get(move || {
        let log = log_for_handler.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            log.push("request_done");
            "done"
        }
    });

    let app = AppBuilder::new()
        .build_state()
        .await
        .merge_router(r2e_core::http::Router::new().route("/slow", slow_route))
        .on_drain(|_state| async move {
            log_for_drain.push("drain");
        })
        .on_stop(|_state| async move {
            log_for_stop.push("stop");
        });

    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    // Open an in-flight request against the slow route.
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    // Give the server a moment to accept and start handling the request.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Trigger shutdown while the request is in flight.
    stop.stop();

    // The in-flight request completes with a full response despite the stop.
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "in-flight request was cut off: {response}"
    );
    assert!(response.contains("done"), "unexpected body: {response}");

    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok());

    // Hook order: drain (at stop time, before the request finished — the
    // server was still serving) → in-flight request completion → on_stop
    // (after the drain completed).
    assert_eq!(log.entries(), vec!["drain", "request_done", "stop"]);
}
