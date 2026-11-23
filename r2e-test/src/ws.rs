use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Default timeout for WS receive operations.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A WebSocket test client connected to a live server.
///
/// Mirrors the `WsStream` API from `r2e-core` for familiarity:
/// `send_text`, `send_json`, `next_text`, `next_json`.
///
/// All receive operations have a configurable timeout to prevent
/// tests from hanging indefinitely.
pub struct WsTestClient {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
    /// The subprotocol the server echoed back in the upgrade response.
    protocol: Option<String>,
}

/// Error type for WS test operations.
#[derive(Debug)]
pub enum WsTestError {
    /// The receive operation timed out.
    Timeout,
    /// The WebSocket connection was closed.
    Closed,
    /// A tungstenite protocol error.
    Protocol(tokio_tungstenite::tungstenite::Error),
    /// JSON serialization/deserialization error.
    Json(r2e_core::json::JsonError),
}

impl std::fmt::Display for WsTestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "ws receive timed out"),
            Self::Closed => write!(f, "ws connection closed"),
            Self::Protocol(e) => write!(f, "ws protocol error: {e}"),
            Self::Json(e) => write!(f, "ws json error: {e}"),
        }
    }
}

impl std::error::Error for WsTestError {}

impl WsTestError {
    /// The status of the HTTP response that refused the handshake, when the
    /// server answered the upgrade request with an ordinary response (a guard
    /// denial, a failed identity, a 404) instead of `101 Switching Protocols`.
    pub fn http_status(&self) -> Option<r2e_core::http::StatusCode> {
        match self {
            Self::Protocol(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                r2e_core::http::StatusCode::from_u16(response.status().as_u16()).ok()
            }
            _ => None,
        }
    }
}

impl WsTestClient {
    /// Connect to a WebSocket endpoint.
    ///
    /// `url` should be a full `ws://` URL, e.g. `ws://127.0.0.1:12345/ws/echo`.
    pub async fn connect(url: &str) -> Result<Self, WsTestError> {
        Self::connect_with_protocols(url, &[]).await
    }

    /// Connect to a WebSocket endpoint, offering `protocols` in
    /// `Sec-WebSocket-Protocol` (in that order, as a browser's
    /// `new WebSocket(url, protocols)` does).
    ///
    /// The subprotocol the server selected is then available from
    /// [`protocol`](Self::protocol). A server that refuses the handshake
    /// before the upgrade (a guard, say) yields an error whose
    /// [`http_status`](WsTestError::http_status) is that response's status.
    pub async fn connect_with_protocols(
        url: &str,
        protocols: &[&str],
    ) -> Result<Self, WsTestError> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
        use tokio_tungstenite::tungstenite::http::HeaderValue;

        let mut request = url.into_client_request().map_err(WsTestError::Protocol)?;
        if !protocols.is_empty() {
            let offered = HeaderValue::from_str(&protocols.join(", "))
                .expect("subprotocols must be valid header text");
            request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, offered);
        }
        let (stream, response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(WsTestError::Protocol)?;
        let protocol = response
            .headers()
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Self {
            stream,
            timeout: DEFAULT_TIMEOUT,
            protocol,
        })
    }

    /// The subprotocol the server selected (the `Sec-WebSocket-Protocol` of
    /// the upgrade response), or `None` when it selected none.
    pub fn protocol(&self) -> Option<&str> {
        self.protocol.as_deref()
    }

    /// Set the timeout for receive operations.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    // ── Send ──

    /// Send a text message.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), WsTestError> {
        let s: String = text.into();
        self.stream
            .send(Message::Text(s.into()))
            .await
            .map_err(WsTestError::Protocol)
    }

    /// Send a JSON-serialized message.
    pub async fn send_json<T: Serialize>(&mut self, data: &T) -> Result<(), WsTestError> {
        let json = r2e_core::json::to_string(data).map_err(WsTestError::Json)?;
        self.send_text(json).await
    }

    /// Send a binary message.
    pub async fn send_binary(&mut self, data: impl Into<bytes::Bytes>) -> Result<(), WsTestError> {
        self.stream
            .send(Message::Binary(data.into()))
            .await
            .map_err(WsTestError::Protocol)
    }

    /// Send a close frame.
    pub async fn close(&mut self) -> Result<(), WsTestError> {
        self.stream.close(None).await.map_err(WsTestError::Protocol)
    }

    // ── Receive ──

    /// Receive the next text message, with timeout.
    /// Skips ping/pong frames. Returns `Err(Timeout)` if no message arrives.
    pub async fn next_text(&mut self) -> Result<String, WsTestError> {
        let fut = async {
            loop {
                match self.stream.next().await {
                    Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
                    Some(Ok(Message::Close(_))) | None => return Err(WsTestError::Closed),
                    Some(Err(e)) => return Err(WsTestError::Protocol(e)),
                    Some(Ok(_)) => continue, // skip ping/pong/binary
                }
            }
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| WsTestError::Timeout)?
    }

    /// Receive the next message and deserialize as JSON, with timeout.
    pub async fn next_json<T: DeserializeOwned>(&mut self) -> Result<T, WsTestError> {
        let text = self.next_text().await?;
        r2e_core::json::from_str(&text).map_err(WsTestError::Json)
    }

    /// Receive the next binary message, with timeout.
    pub async fn next_binary(&mut self) -> Result<Vec<u8>, WsTestError> {
        let fut = async {
            loop {
                match self.stream.next().await {
                    Some(Ok(Message::Binary(data))) => return Ok(data.to_vec()),
                    Some(Ok(Message::Close(_))) | None => return Err(WsTestError::Closed),
                    Some(Err(e)) => return Err(WsTestError::Protocol(e)),
                    Some(Ok(_)) => continue,
                }
            }
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| WsTestError::Timeout)?
    }

    /// Wait for the server to end the connection, with timeout.
    ///
    /// The connection is over when a close frame arrives, the stream ends, or
    /// the transport fails (a server that drops the socket without a closing
    /// handshake — what a refused oversized message looks like). Ping/pong
    /// frames are skipped.
    ///
    /// # Panics
    ///
    /// Panics when a text or binary message arrives instead, or when the
    /// connection is still open after the timeout.
    pub async fn assert_closed(&mut self) {
        let fut = async {
            loop {
                match self.stream.next().await {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                    Some(Ok(Message::Text(text))) => {
                        panic!("Expected the connection to close, but received text: {text}")
                    }
                    Some(Ok(Message::Binary(data))) => panic!(
                        "Expected the connection to close, but received {} binary bytes",
                        data.len()
                    ),
                    Some(Ok(_)) => continue,
                }
            }
        };
        if tokio::time::timeout(self.timeout, fut).await.is_err() {
            panic!(
                "Expected the connection to close within {:?}, but it is still open",
                self.timeout
            );
        }
    }

    /// Assert that no message arrives within the given duration.
    pub async fn assert_no_message(&mut self, wait: Duration) {
        let result = tokio::time::timeout(wait, self.stream.next()).await;
        assert!(
            result.is_err(),
            "Expected no message within {wait:?}, but received one",
        );
    }
}
