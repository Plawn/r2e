#![cfg(feature = "proxy")]

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use r2e_core::http::{Body, Response};
use r2e_core::prelude::*;
use r2e_core::proxy::ForwardProxyLayer;
use r2e_core::tunnel::TcpTunnel;

// ── Test state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct ProxyState;

// ── Test controller with #[connect] ─────────────────────────────────────

#[derive(Controller)]
#[controller(state = ProxyState)]
struct ProxyController;

#[routes]
impl ProxyController {
    #[connect]
    async fn handle_tunnel(&self, tunnel: TcpTunnel) {
        let target = tunnel.target();
        if let Ok(mut upstream) = tokio::net::TcpStream::connect(&target).await {
            let _ = tunnel.pipe(&mut upstream).await;
        }
    }
}

// ── Unit test: connect_handler is generated ─────────────────────────────

#[test]
fn connect_handler_is_generated() {
    let handler = <ProxyController as ControllerTrait<ProxyState>>::connect_handler();
    assert!(handler.is_some(), "connect_handler() should return Some");
}

// ── Integration test: CONNECT tunnel ────────────────────────────────────

#[r2e_core::test]
async fn connect_tunnel_bidirectional() {
    let echo_ready = Arc::new(Notify::new());
    let echo_ready2 = echo_ready.clone();

    // Spawn a simple echo server
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        echo_ready2.notify_one();
        let (mut socket, _) = echo_listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            socket.write_all(&buf[..n]).await.unwrap();
        }
    });
    echo_ready.notified().await;

    // Build the proxy app
    let app = r2e_core::builder::AppBuilder::new()
        .with_state(ProxyState)
        .register_controller::<ProxyController>();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = listener.local_addr().unwrap();

    let app = app.prepare(&proxy_addr.to_string());
    tokio::spawn(async move {
        app.run_with_listener(listener).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Connect to the proxy with raw TCP and send a CONNECT request
    let mut stream = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();

    let connect_req = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
        echo_addr, echo_addr
    );
    stream.write_all(connect_req.as_bytes()).await.unwrap();

    // Read the 200 response
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected 200, got: {response}"
    );

    // Now the connection is upgraded — send data through the tunnel
    stream.write_all(b"hello from client").await.unwrap();

    let mut echo_buf = vec![0u8; 64];
    let n = stream.read(&mut echo_buf).await.unwrap();
    assert_eq!(&echo_buf[..n], b"hello from client");
}

// ── Integration test: forward proxy ────────────────────────────────────

#[r2e_core::test]
async fn forward_proxy_intercepts_absolute_uri() {
    let app = r2e_core::builder::AppBuilder::new()
        .with_state(ProxyState)
        .register_controller::<ProxyController>()
        .with_layer_fn(|router| {
            let handler: r2e_core::proxy::ForwardHandlerFn = Arc::new(|_req| {
                Box::pin(async {
                    Response::builder()
                        .status(200)
                        .body(Body::from("forwarded"))
                        .unwrap()
                })
            });
            router.layer(ForwardProxyLayer::new(handler))
        });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = listener.local_addr().unwrap();

    let app = app.prepare(&proxy_addr.to_string());
    tokio::spawn(async move {
        app.run_with_listener(listener).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send a forward-proxy request with an absolute URI
    let mut stream = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
    let req = "GET http://example.com/some/path HTTP/1.1\r\nHost: example.com\r\n\r\n";
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    assert!(
        response.contains("200"),
        "expected 200 for forward proxy, got: {response}"
    );
    assert!(
        response.contains("forwarded"),
        "expected body 'forwarded', got: {response}"
    );
}
