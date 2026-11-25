//! `WsTestClient` against a scripted raw WebSocket peer: each helper must
//! really send, really receive, and its assertions must fail on wrong input.
#![cfg(feature = "ws")]

use std::future::Future;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use r2e_test::WsTestClient;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

type Peer = WebSocketStream<TcpStream>;

/// Accept one connection on a random port, run `script` on it, return the
/// `ws://` URL. The handshake echoes the LAST offered subprotocol.
async fn peer<F, Fut>(script: F) -> String
where
    F: FnOnce(Peer) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    r2e_core::rt::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let select_last = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
            if let Some(offered) = req.headers().get(SEC_WEBSOCKET_PROTOCOL) {
                let last = offered
                    .to_str()
                    .unwrap()
                    .rsplit(", ")
                    .next()
                    .unwrap()
                    .to_owned();
                resp.headers_mut()
                    .insert(SEC_WEBSOCKET_PROTOCOL, last.parse().unwrap());
            }
            Ok(resp)
        };
        let ws = tokio_tungstenite::accept_hdr_async(tcp, select_last)
            .await
            .unwrap();
        script(ws).await;
    });
    format!("ws://{addr}/")
}

/// A peer that echoes every text/binary message and reports the close frame.
async fn echo_peer() -> (String, oneshot::Receiver<()>) {
    let (closed_tx, closed_rx) = oneshot::channel();
    let url = peer(move |mut ws| async move {
        while let Some(Ok(msg)) = ws.next().await {
            match msg {
                Message::Text(_) | Message::Binary(_) => ws.send(msg).await.unwrap(),
                Message::Close(_) => {
                    let _ = closed_tx.send(());
                    return;
                }
                _ => {}
            }
        }
    })
    .await;
    (url, closed_rx)
}

#[r2e::test]
async fn send_text_and_next_text_round_trip() {
    let (url, _closed) = echo_peer().await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.send_text("hello").await.unwrap();
    assert_eq!(client.next_text().await.unwrap(), "hello");
}

#[r2e::test]
async fn send_json_and_next_json_round_trip() {
    let (url, _closed) = echo_peer().await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client
        .send_json(&serde_json::json!({"n": 1}))
        .await
        .unwrap();
    let back: serde_json::Value = client.next_json().await.unwrap();
    assert_eq!(back, serde_json::json!({"n": 1}));
}

#[r2e::test]
async fn send_binary_and_next_binary_round_trip() {
    let (url, _closed) = echo_peer().await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.send_binary(vec![7u8, 8, 9]).await.unwrap();
    assert_eq!(client.next_binary().await.unwrap(), vec![7u8, 8, 9]);
}

#[r2e::test]
async fn close_sends_a_close_frame() {
    let (url, closed) = echo_peer().await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.close().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), closed)
        .await
        .expect("the peer never saw a close frame")
        .unwrap();
}

#[r2e::test]
async fn next_text_and_next_binary_skip_other_frames() {
    let url = peer(|mut ws| async move {
        ws.send(Message::Ping(vec![1u8].into())).await.unwrap();
        ws.send(Message::Binary(vec![1u8].into())).await.unwrap();
        ws.send(Message::Text("first".into())).await.unwrap();
        ws.send(Message::Binary(vec![2u8, 3].into())).await.unwrap();
        // Keep the socket open until the client is done.
        let _ = ws.next().await;
    })
    .await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    assert_eq!(client.next_text().await.unwrap(), "first");
    assert_eq!(client.next_binary().await.unwrap(), vec![2u8, 3]);
}

#[r2e::test]
async fn protocol_is_the_one_the_server_selected() {
    let (url, _closed) = echo_peer().await;
    let client = WsTestClient::connect_with_protocols(&url, &["chat.v1", "chat.v2"])
        .await
        .unwrap();
    assert_eq!(client.protocol(), Some("chat.v2"));
}

#[r2e::test]
async fn protocol_is_none_when_none_was_offered() {
    let (url, _closed) = echo_peer().await;
    let client = WsTestClient::connect(&url).await.unwrap();
    assert_eq!(client.protocol(), None);
}

#[r2e::test]
async fn refused_handshake_exposes_the_http_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    r2e_core::rt::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let refuse = |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
            let mut denied = ErrorResponse::new(Some("denied".into()));
            *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(denied)
        };
        let _ = tokio_tungstenite::accept_hdr_async(tcp, refuse).await;
    });
    let err = match WsTestClient::connect(&format!("ws://{addr}/")).await {
        Ok(_) => panic!("the handshake should have been refused"),
        Err(e) => e,
    };
    assert_eq!(
        err.http_status(),
        Some(r2e_core::http::StatusCode::FORBIDDEN)
    );
}

#[r2e::test]
async fn http_status_is_none_for_other_errors() {
    assert_eq!(r2e_test::WsTestError::Timeout.http_status(), None);
}

#[r2e::test]
async fn assert_closed_passes_when_the_server_closes() {
    let url = peer(|mut ws| async move {
        ws.close(None).await.unwrap();
    })
    .await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.assert_closed().await;
}

#[r2e::test]
#[should_panic(expected = "Expected the connection to close, but received text: still here")]
async fn assert_closed_fails_on_a_text_message() {
    let url = peer(|mut ws| async move {
        ws.send(Message::Text("still here".into())).await.unwrap();
        let _ = ws.next().await;
    })
    .await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.assert_closed().await;
}

#[r2e::test]
#[should_panic(expected = "but it is still open")]
async fn assert_closed_fails_when_the_connection_stays_open() {
    let url = peer(|mut ws| async move {
        let _ = ws.next().await;
    })
    .await;
    let mut client = WsTestClient::connect(&url)
        .await
        .unwrap()
        .with_timeout(Duration::from_millis(100));
    client.assert_closed().await;
}

#[r2e::test]
async fn assert_no_message_passes_on_a_silent_peer() {
    let url = peer(|mut ws| async move {
        let _ = ws.next().await;
    })
    .await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.assert_no_message(Duration::from_millis(100)).await;
}

#[r2e::test]
#[should_panic(expected = "Expected no message within")]
async fn assert_no_message_fails_when_a_message_arrives() {
    let url = peer(|mut ws| async move {
        ws.send(Message::Text("surprise".into())).await.unwrap();
        let _ = ws.next().await;
    })
    .await;
    let mut client = WsTestClient::connect(&url).await.unwrap();
    client.assert_no_message(Duration::from_secs(2)).await;
}
