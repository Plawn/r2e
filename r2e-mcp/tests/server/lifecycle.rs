//! Serve lifecycle over live TCP: one session map shared across
//! SO_REUSEPORT workers, and open SSE streams terminated on
//! `StopHandle::stop()` (the plugin's dedicated cancel token relayed from
//! the app shutdown token) so graceful drain does not hang.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use r2e_core::{AppBuilder, R2eConfig};
use r2e_mcp::{AppBuilderMcpExt, McpServer};

use crate::fixtures::{Calc, CallLog, FixtureTools};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Blocking HTTP/1.1 POST to the MCP endpoint over a fresh connection;
/// returns the raw response (headers + body, read to EOF).
fn blocking_post(addr: &str, session: Option<&str>, body: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let session_header = session
        .map(|sid| format!("Mcp-Session-Id: {sid}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         {session_header}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

/// Case-insensitive header lookup in a raw HTTP response.
fn header_value(response: &str, name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    response
        .split("\r\n\r\n")
        .next()?
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim().to_ascii_lowercase() == lower).then(|| value.trim().to_string())
        })
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lifecycle","version":"0"}}}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sharded_serving_shares_sessions_and_stop_terminates_sse_streams() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let yaml = format!("server:\n  workers: 2\n  port: {port}\n");

    let app = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(&yaml).unwrap())
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .prepare(&addr);
    let stop = app.stop_handle();

    let server = r2e_core::rt::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    // Wait for readiness AND create a session in one step (retry initialize
    // until the listener answers 200).
    let ready_addr = addr.clone();
    let session = r2e_core::rt::spawn_blocking(move || {
        for _ in 0..100 {
            if let Ok(response) = blocking_post(&ready_addr, None, INITIALIZE) {
                if response.starts_with("HTTP/1.1 200") {
                    return header_value(&response, "mcp-session-id")
                        .expect("initialize answered 200 without a session id");
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("server did not become ready");
    })
    .await
    .unwrap();

    // Drive the session over FRESH connections — with 2 SO_REUSEPORT workers
    // these land on arbitrary workers, so this only works because all
    // workers share one session manager and dispatch table.
    let call_addr = addr.clone();
    let sid = session.clone();
    let responses = r2e_core::rt::spawn_blocking(move || {
        let initialized = blocking_post(
            &call_addr,
            Some(&sid),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .unwrap();
        let mut calls = Vec::new();
        for id in 0..4 {
            let body = format!(
                r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"add","arguments":{{"a":2.0,"b":3.0}}}}}}"#,
                id + 10
            );
            calls.push(blocking_post(&call_addr, Some(&sid), &body).unwrap());
        }
        (initialized, calls)
    })
    .await
    .unwrap();
    assert!(
        responses.0.starts_with("HTTP/1.1 202"),
        "notifications/initialized: {}",
        responses.0
    );
    for call in &responses.1 {
        assert!(call.starts_with("HTTP/1.1 200"), "{call}");
        assert!(
            call.contains(r#""value":5"#),
            "tools/call result missing: {call}"
        );
    }

    // Open a standalone SSE stream on the session and LEAVE IT OPEN — this
    // is exactly what would hang graceful drain without the plugin's cancel
    // relay.
    let sse_addr = addr.clone();
    let sid = session.clone();
    let mut sse = r2e_core::rt::spawn_blocking(move || {
        let mut stream = TcpStream::connect(&sse_addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = format!(
            "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nMcp-Session-Id: {sid}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).unwrap();
        // Read until the response head is complete; the stream then stays
        // open (SSE).
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 200"), "SSE GET failed: {head}");
        stream
    })
    .await
    .unwrap();

    // Programmatic stop: the app shutdown token is relayed to the MCP
    // transport token, which terminates the open SSE stream — drain must
    // complete well within the timeout.
    stop.stop();
    let joined = r2e_core::rt::timeout(Duration::from_secs(10), server).await;
    match joined {
        Ok(Ok(Ok(()))) => {}
        other => panic!("server did not stop cleanly: {other:?}"),
    }

    // The held SSE socket was closed by the server.
    let eof = r2e_core::rt::spawn_blocking(move || {
        let mut rest = Vec::new();
        sse.read_to_end(&mut rest).map(|_| ())
    })
    .await
    .unwrap();
    assert!(eof.is_ok(), "SSE stream not closed on stop: {eof:?}");
}
