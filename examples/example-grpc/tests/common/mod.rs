//! Helpers shared by the gRPC integration tests (each test file is its own
//! crate; they opt in with `mod common;`).

#![allow(dead_code)] // each test crate uses a subset of these helpers

use std::time::{Duration, Instant};

/// Pick a free TCP port (bind to :0, read the port, release). The tiny
/// reuse window before the server binds it is acceptable for tests.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Connect a tonic channel with a retry deadline, so a test fails with a
/// clear panic (instead of hanging) when the server never comes up — which
/// is exactly what happened before the serve path was wired.
pub async fn connect_channel(port: u16) -> tonic::transport::Channel {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
        {
            Ok(channel) => return channel,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "gRPC server did not come up on port {port}: {e}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Stop the server via its `StopHandle` and assert `run()` terminated
/// cleanly — the real graceful-shutdown path, including the awaited gRPC
/// drain on the separate-port transport.
pub async fn stop_and_await_clean(
    stop: r2e::prelude::StopHandle,
    server: tokio::task::JoinHandle<Result<(), String>>,
) {
    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s after StopHandle::stop()")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");
}
