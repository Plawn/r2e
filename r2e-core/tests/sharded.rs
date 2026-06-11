//! Tests for SO_REUSEPORT sharded serving (`server.workers`).
//!
//! The integration test is gated to the platforms that support
//! `SO_REUSEPORT` (mirroring `socket2::Socket::set_reuse_port`'s cfg). The
//! unit-level parsing tests run everywhere.

use r2e_core::builder::AppBuilder;
use r2e_core::config::R2eConfig;
use r2e_core::sharded::parse_workers;

// ── Unit: parse_workers ─────────────────────────────────────────────────────

#[test]
fn parse_workers_absent_is_none() {
    let config = R2eConfig::from_yaml_str("server:\n  port: 3000\n").unwrap();
    assert_eq!(parse_workers(Some(&config)).unwrap(), None);
}

#[test]
fn parse_workers_no_config_is_none() {
    assert_eq!(parse_workers(None).unwrap(), None);
}

#[test]
fn parse_workers_positive_integer() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 4\n").unwrap();
    assert_eq!(parse_workers(Some(&config)).unwrap(), Some(4));
}

#[test]
fn parse_workers_one() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 1\n").unwrap();
    assert_eq!(parse_workers(Some(&config)).unwrap(), Some(1));
}

#[test]
fn parse_workers_per_core() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: \"per-core\"\n").unwrap();
    let n = parse_workers(Some(&config)).unwrap().unwrap();
    let expected = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    assert_eq!(n, expected);
    assert!(n >= 1);
}

#[test]
fn parse_workers_zero_is_error() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 0\n").unwrap();
    let err = parse_workers(Some(&config)).unwrap_err();
    assert!(err.contains("positive integer"), "got: {err}");
}

#[test]
fn parse_workers_negative_is_error() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: -2\n").unwrap();
    let err = parse_workers(Some(&config)).unwrap_err();
    assert!(err.contains("positive integer"), "got: {err}");
}

#[test]
fn parse_workers_invalid_string_is_error() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: \"lots\"\n").unwrap();
    let err = parse_workers(Some(&config)).unwrap_err();
    assert!(err.contains("per-core"), "got: {err}");
}

#[test]
fn parse_workers_above_cap_is_error() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 100000000000\n").unwrap();
    let err = parse_workers(Some(&config)).unwrap_err();
    assert!(err.contains("at most"), "got: {err}");
}

// ── PreparedApp::workers() accessor ─────────────────────────────────────────

#[test]
fn prepared_app_workers_accessor_ok() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 2\n").unwrap();
    let app = AppBuilder::new()
        .with_config(config)
        .with_state(())
        .prepare("127.0.0.1:0");
    assert_eq!(app.workers().unwrap(), Some(2));
}

#[test]
fn prepared_app_workers_accessor_err() {
    let config = R2eConfig::from_yaml_str("server:\n  workers: 0\n").unwrap();
    let app = AppBuilder::new()
        .with_config(config)
        .with_state(())
        .prepare("127.0.0.1:0");
    assert!(app.workers().is_err());
}

#[test]
fn prepared_app_workers_default_none() {
    let app = AppBuilder::new().with_state(()).prepare("127.0.0.1:0");
    assert_eq!(app.workers().unwrap(), None);
}

// ── Integration: sharded serving (supported platforms only) ─────────────────

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod integration {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Pick a concrete free port (port 0 would give each worker a *different*
    /// ephemeral port), then immediately release it.
    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        port
    }

    /// Send a single HTTP/1.1 `GET /ping` on a fresh connection and assert the
    /// response is `200` with the expected body.
    async fn request_ping(addr: &str) -> Result<(), String> {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        stream
            .write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .map_err(|e| format!("write: {e}"))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        let text = String::from_utf8_lossy(&buf);
        if !text.starts_with("HTTP/1.1 200") {
            return Err(format!("unexpected status line: {:?}", text.lines().next()));
        }
        if !text.contains("pong") {
            return Err(format!("missing body 'pong' in response: {text}"));
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sharded_serves_requests_on_fresh_connections() {
        use r2e_core::http::routing::get;

        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let yaml = format!("server:\n  workers: 2\n  port: {port}\n");
        let config = R2eConfig::from_yaml_str(&yaml).unwrap();

        let app = AppBuilder::new()
            .with_config(config)
            .with_state(())
            .register_routes(
                r2e_core::http::Router::new().route("/ping", get(|| async { "pong" })),
            )
            .prepare(&addr);

        // Sanity: the workers config parsed to Some(2).
        assert_eq!(app.workers().unwrap(), Some(2));

        // `run()` installs a signal handler and serves until SIGINT/SIGTERM.
        // Drive it on a background task; we trigger a graceful shutdown via
        // SIGINT once the assertions are done so the worker threads exit and
        // `run()` returns cleanly (aborting the future would leak the worker
        // OS threads and hang the test binary at exit).
        let server = tokio::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

        // Wait until the server is accepting connections.
        let mut ready = false;
        for _ in 0..100 {
            if request_ping(&addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ready, "server did not become ready in time");

        // ~20 sequential requests on fresh connections must all succeed. We do
        // NOT assert distribution across workers (kernel-dependent).
        for i in 0..20 {
            request_ping(&addr)
                .await
                .unwrap_or_else(|e| panic!("request {i} failed: {e}"));
        }

        // Trigger graceful shutdown (SIGINT → tokio Ctrl-C handler in
        // `run()`), then confirm `run()` returns within a bounded time.
        // The short sleep gives the spawned shutdown task time to reach
        // `ctrl_c().await` — serving readiness alone does not strictly
        // happen-after handler installation.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // SAFETY: raising SIGINT on our own process; tokio has a handler.
        unsafe {
            libc::raise(libc::SIGINT);
        }

        let joined = tokio::time::timeout(Duration::from_secs(10), server).await;
        match joined {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => panic!("server returned error: {e}"),
            Ok(Err(join_err)) => panic!("server task join error: {join_err}"),
            Err(_) => panic!("server did not shut down within 10s after SIGINT"),
        }
    }

    /// Port 0 must work under sharding: the first listener gets a
    /// kernel-assigned ephemeral port and the remaining workers bind that
    /// same concrete port via SO_REUSEPORT. A failure of worker 2 to bind
    /// the resolved port would surface as an `Err` here.
    #[test]
    fn sharded_port_zero_shares_one_ephemeral_port() {
        use tokio_util::sync::CancellationToken;

        let token = CancellationToken::new();
        let cancel = token.clone();
        let handle = std::thread::spawn(move || {
            r2e_core::sharded::serve_sharded(
                r2e_core::http::Router::new().with_state(()),
                &["127.0.0.1:0".parse().unwrap()],
                2,
                true,
                token,
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        cancel.cancel();
        let res = handle.join().expect("serve_sharded thread panicked");
        res.expect("serve_sharded with port 0 should bind both workers on one port");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sharded_run_rejects_invalid_workers() {
        let yaml = "server:\n  workers: 0\n";
        let config = R2eConfig::from_yaml_str(yaml).unwrap();
        let port = free_port();
        let app = AppBuilder::new()
            .with_config(config)
            .with_state(())
            .prepare(&format!("127.0.0.1:{port}"));
        let err = app.run().await.expect_err("run() should reject workers=0");
        assert!(
            err.to_string().contains("positive integer"),
            "unexpected error: {err}"
        );
    }
}
