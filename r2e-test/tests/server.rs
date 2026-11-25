//! `TestServer` URLs point at the live listener, and dropping it stops it.

use std::time::Duration;

use r2e_core::http::routing::get;
use r2e_core::http::Routes;
use r2e_test::TestServer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn routes() -> Routes {
    Routes::new().route("/ping", get(|_, _| async { "pong" }))
}

/// A raw `GET /ping` over a fresh connection; the response text.
async fn raw_get(addr: std::net::SocketAddr) -> std::io::Result<String> {
    let mut tcp = TcpStream::connect(addr).await?;
    tcp.write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await?;
    let mut out = String::new();
    tcp.read_to_string(&mut out).await?;
    Ok(out)
}

#[r2e::test]
async fn url_and_ws_url_name_the_listener() {
    let server = TestServer::new(routes()).await;
    let addr = server.addr();
    assert_eq!(server.url(), format!("http://{addr}"));
    #[cfg(feature = "ws")]
    assert_eq!(server.ws_url(), format!("ws://{addr}"));
    assert!(raw_get(addr).await.unwrap().ends_with("pong"));
}

#[r2e::test]
async fn dropping_the_server_stops_it() {
    let server = TestServer::new(routes()).await;
    let addr = server.addr();
    assert!(raw_get(addr).await.unwrap().ends_with("pong"));
    drop(server);
    tokio::time::timeout(Duration::from_secs(5), async {
        while raw_get(addr).await.is_ok_and(|r| r.ends_with("pong")) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the server still answers after being dropped");
}
