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
    Json(serde_json::Error),
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

impl WsTestClient {
    /// Connect to a WebSocket endpoint.
    ///
    /// `url` should be a full `ws://` URL, e.g. `ws://127.0.0.1:12345/ws/echo`.
    pub async fn connect(url: &str) -> Result<Self, WsTestError> {
        let (stream, _response) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(WsTestError::Protocol)?;
        Ok(Self {
            stream,
            timeout: DEFAULT_TIMEOUT,
        })
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
        let json = serde_json::to_string(data).map_err(WsTestError::Json)?;
        self.send_text(json).await
    }

    /// Send a binary message.
    pub async fn send_binary(&mut self, data: impl Into<Vec<u8>>) -> Result<(), WsTestError> {
        let bytes: Vec<u8> = data.into();
        self.stream
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(WsTestError::Protocol)
    }

    /// Send a close frame.
    pub async fn close(&mut self) -> Result<(), WsTestError> {
        self.stream
            .close(None)
            .await
            .map_err(WsTestError::Protocol)
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
        serde_json::from_str(&text).map_err(WsTestError::Json)
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

    /// Assert that no message arrives within the given duration.
    pub async fn assert_no_message(&mut self, wait: Duration) {
        let result = tokio::time::timeout(wait, self.stream.next()).await;
        assert!(
            result.is_err(),
            "Expected no message within {wait:?}, but received one",
        );
    }
}
