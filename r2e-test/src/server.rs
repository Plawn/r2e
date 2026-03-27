use std::net::SocketAddr;

use r2e_core::http::{serve, Router};
use tokio::net::TcpListener;

/// A test server running on a random local port.
///
/// The server is spawned as a background tokio task and is shut down
/// when the `TestServer` is dropped.
pub struct TestServer {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestServer {
    /// Spawn a test server from a `Router`, binding to a random local port.
    pub async fn new(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind test server");
        let addr = listener.local_addr().expect("failed to get local addr");

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            serve(listener, router.into_make_service())
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("test server failed");
        });

        Self {
            addr,
            shutdown: Some(shutdown_tx),
        }
    }

    /// Returns the socket address of the running server.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the base URL for HTTP requests (e.g., `http://127.0.0.1:54321`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Returns the base URL for WebSocket requests (e.g., `ws://127.0.0.1:54321`).
    #[cfg(feature = "ws")]
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Connect to a WebSocket endpoint on this server.
    ///
    /// `path` is relative to the server root, e.g., `/chat/room1`.
    #[cfg(feature = "ws")]
    pub async fn ws(&self, path: &str) -> crate::ws::WsTestClient {
        let url = format!("ws://{}{}", self.addr, path);
        crate::ws::WsTestClient::connect(&url)
            .await
            .expect("failed to connect WebSocket")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}
