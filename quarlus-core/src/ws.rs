//! WebSocket utilities — ergonomic wrappers, handler trait, and broadcaster.
//!
//! # WsStream
//!
//! An ergonomic wrapper around Axum's [`WebSocket`](axum::extract::ws::WebSocket)
//! with typed helpers for text, JSON, and binary messages.
//!
//! # WsHandler
//!
//! An optional lifecycle trait for structured WebSocket handling. The framework
//! manages the message loop; you implement `on_connect`, `on_message`, `on_close`.
//!
//! # WsBroadcaster / WsRooms
//!
//! Multi-client broadcast utilities for chat rooms, notifications, etc.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::broadcast;

// ── WsError ──────────────────────────────────────────────────────────────

/// Errors from WebSocket operations.
#[derive(Debug)]
pub enum WsError {
    Send(axum::Error),
    Recv(axum::Error),
    Json(serde_json::Error),
    Closed,
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsError::Send(e) => write!(f, "ws send: {e}"),
            WsError::Recv(e) => write!(f, "ws recv: {e}"),
            WsError::Json(e) => write!(f, "ws json: {e}"),
            WsError::Closed => write!(f, "ws closed"),
        }
    }
}

impl std::error::Error for WsError {}

// ── WsStream ─────────────────────────────────────────────────────────────

/// Ergonomic wrapper around Axum's WebSocket with typed helpers.
pub struct WsStream {
    inner: WebSocket,
}

impl crate::http::ws::IsWebSocket for WsStream {}

impl WsStream {
    /// Wrap a raw Axum WebSocket.
    pub fn new(socket: WebSocket) -> Self {
        Self { inner: socket }
    }

    // ── Send ──

    /// Send a raw message.
    pub async fn send(&mut self, msg: Message) -> Result<(), WsError> {
        self.inner.send(msg).await.map_err(WsError::Send)
    }

    /// Send a text message.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), WsError> {
        self.send(Message::Text(text.into().into())).await
    }

    /// Send a JSON-serialized message.
    pub async fn send_json<T: Serialize>(&mut self, data: &T) -> Result<(), WsError> {
        let json = serde_json::to_string(data).map_err(WsError::Json)?;
        self.send_text(json).await
    }

    /// Send a binary message.
    pub async fn send_binary(&mut self, data: Vec<u8>) -> Result<(), WsError> {
        self.send(Message::Binary(data.into())).await
    }

    // ── Receive ──

    /// Receive the next message, or `None` if the connection is closed.
    pub async fn next(&mut self) -> Option<Result<Message, WsError>> {
        use futures_core::Stream;
        use std::pin::Pin;
        use std::task::Poll;

        std::future::poll_fn(|cx| {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(msg))) => Poll::Ready(Some(Ok(msg))),
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(WsError::Recv(e)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    /// Receive the next text message, skipping non-text messages.
    pub async fn next_text(&mut self) -> Option<Result<String, WsError>> {
        loop {
            match self.next().await? {
                Ok(Message::Text(text)) => return Some(Ok(text.to_string())),
                Ok(Message::Close(_)) => return None,
                Err(e) => return Some(Err(e)),
                _ => continue,
            }
        }
    }

    /// Receive the next message and deserialize as JSON.
    pub async fn next_json<T: DeserializeOwned>(&mut self) -> Option<Result<T, WsError>> {
        let text = match self.next_text().await? {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };
        Some(serde_json::from_str(&text).map_err(WsError::Json))
    }

    /// Process messages in a loop with a callback. Returns when the connection closes.
    pub async fn on_each<F, Fut>(&mut self, mut handler: F)
    where
        F: FnMut(Message) -> Fut,
        Fut: Future<Output = Option<Message>>,
    {
        while let Some(Ok(msg)) = self.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            if let Some(reply) = handler(msg).await {
                if self.send(reply).await.is_err() {
                    break;
                }
            }
        }
    }

    /// Unwrap into the raw Axum WebSocket (escape hatch).
    pub fn into_inner(self) -> WebSocket {
        self.inner
    }
}

// ── WsHandler trait ──────────────────────────────────────────────────────

/// Optional lifecycle trait for structured WebSocket handling.
///
/// The framework runs the message loop; you implement the callbacks.
#[allow(unused_variables)]
pub trait WsHandler: Send + 'static {
    /// Called when the WebSocket connection is established.
    fn on_connect(&mut self, ws: &mut WsStream) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Called for each received message.
    fn on_message(&mut self, ws: &mut WsStream, msg: Message) -> impl Future<Output = ()> + Send;

    /// Called when the connection closes.
    fn on_close(&mut self) -> impl Future<Output = ()> + Send {
        async {}
    }
}

/// Run a WsHandler on a WsStream. Called by generated code.
pub async fn run_ws_handler(mut ws: WsStream, mut handler: impl WsHandler) {
    handler.on_connect(&mut ws).await;
    while let Some(Ok(msg)) = ws.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
        handler.on_message(&mut ws, msg).await;
    }
    handler.on_close().await;
}

// ── WsBroadcaster ────────────────────────────────────────────────────────

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// Broadcast message wrapper with optional sender exclusion.
#[derive(Clone)]
struct BroadcastMessage {
    data: Arc<Message>,
    #[allow(dead_code)]
    sender_id: Option<u64>,
}

/// Multi-client WebSocket broadcaster.
///
/// Clone + Send + Sync — injectable via `#[inject]`.
#[derive(Clone)]
pub struct WsBroadcaster {
    tx: broadcast::Sender<BroadcastMessage>,
}

impl WsBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Broadcast a text message to all subscribers.
    pub fn send_text(&self, text: impl Into<String>) {
        let s: String = text.into();
        let _ = self.tx.send(BroadcastMessage {
            data: Arc::new(Message::Text(s.into())),
            sender_id: None,
        });
    }

    /// Broadcast a JSON-serialized message.
    pub fn send_json<T: Serialize>(&self, data: &T) -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(data)?;
        self.send_text(json);
        Ok(())
    }

    /// Broadcast a raw message.
    pub fn send(&self, msg: Message) {
        let _ = self.tx.send(BroadcastMessage {
            data: Arc::new(msg),
            sender_id: None,
        });
    }

    /// Create a receiver for a new client.
    pub fn subscribe(&self) -> WsBroadcastReceiver {
        WsBroadcastReceiver {
            rx: self.tx.subscribe(),
            client_id: NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// Receiver end of a WsBroadcaster subscription.
pub struct WsBroadcastReceiver {
    rx: broadcast::Receiver<BroadcastMessage>,
    #[allow(dead_code)]
    client_id: u64,
}

impl WsBroadcastReceiver {
    /// Receive the next broadcast message.
    pub async fn recv(&mut self) -> Option<Message> {
        loop {
            match self.rx.recv().await {
                Ok(msg) => return Some((*msg.data).clone()),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }
}

// ── WsRooms ──────────────────────────────────────────────────────────────

/// Named room manager for WebSocket broadcasting.
///
/// Clone + Send + Sync — injectable via `#[inject]`.
#[derive(Clone)]
pub struct WsRooms {
    rooms: Arc<DashMap<String, WsBroadcaster>>,
    capacity: usize,
}

impl WsRooms {
    /// Create a new room manager with the given per-room channel capacity.
    pub fn new(capacity_per_room: usize) -> Self {
        Self {
            rooms: Arc::new(DashMap::new()),
            capacity: capacity_per_room,
        }
    }

    /// Get or create a broadcaster for the given room name.
    pub fn room(&self, name: &str) -> WsBroadcaster {
        self.rooms
            .entry(name.to_string())
            .or_insert_with(|| WsBroadcaster::new(self.capacity))
            .clone()
    }

    /// Remove a room.
    pub fn remove(&self, name: &str) {
        self.rooms.remove(name);
    }
}
