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

use std::borrow::Borrow;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::http::ws::{Message, WebSocket};
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::broadcast;

// ── WsError ──────────────────────────────────────────────────────────────

/// Errors from WebSocket operations.
#[derive(Debug)]
pub enum WsError {
    Send(crate::http::Error),
    Recv(crate::http::Error),
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
        use tokio_stream::StreamExt;
        self.inner.next().await.map(|r| r.map_err(WsError::Recv))
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
    ///
    /// Decodes directly from the text frame's backing bytes to avoid the
    /// intermediate `String` allocation of the naive `next_text` + `from_str` path.
    pub async fn next_json<T: DeserializeOwned>(&mut self) -> Option<Result<T, WsError>> {
        loop {
            match self.next().await? {
                Ok(Message::Text(bytes)) => {
                    return Some(serde_json::from_slice(bytes.as_bytes()).map_err(WsError::Json));
                }
                Ok(Message::Close(_)) => return None,
                Err(e) => return Some(Err(e)),
                _ => continue,
            }
        }
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

/// Broadcast message wrapper with optional sender exclusion.
#[derive(Clone)]
struct BroadcastMessage {
    data: Arc<Message>,
    sender_id: Option<u64>,
}

/// Multi-client WebSocket broadcaster.
///
/// Clone + Send + Sync — injectable via `#[inject]`.
#[derive(Clone)]
pub struct WsBroadcaster {
    tx: broadcast::Sender<BroadcastMessage>,
    /// Per-broadcaster client id counter. Scoped to this instance so
    /// tests and independent broadcasters get fresh, predictable ids.
    next_client_id: Arc<AtomicU64>,
}

impl WsBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            next_client_id: Arc::new(AtomicU64::new(1)),
        }
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

    /// Broadcast a text message, excluding the sender.
    pub fn send_text_from(&self, sender_id: u64, text: impl Into<String>) {
        let s: String = text.into();
        let _ = self.tx.send(BroadcastMessage {
            data: Arc::new(Message::Text(s.into())),
            sender_id: Some(sender_id),
        });
    }

    /// Broadcast a JSON message, excluding the sender.
    pub fn send_json_from<T: Serialize>(
        &self,
        sender_id: u64,
        data: &T,
    ) -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(data)?;
        self.send_text_from(sender_id, json);
        Ok(())
    }

    /// Broadcast a raw message, excluding the sender.
    pub fn send_from(&self, sender_id: u64, msg: Message) {
        let _ = self.tx.send(BroadcastMessage {
            data: Arc::new(msg),
            sender_id: Some(sender_id),
        });
    }

    /// Create a receiver for a new client.
    pub fn subscribe(&self) -> WsBroadcastReceiver {
        WsBroadcastReceiver {
            rx: self.tx.subscribe(),
            client_id: self.next_client_id.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Number of active subscribers on this broadcaster.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Returns true when no sent message is still pending for any subscriber.
    ///
    /// Matches [`tokio::sync::broadcast::Sender::is_empty`] — reflects the slowest
    /// receiver, not a sum across receivers.
    pub fn is_empty(&self) -> bool {
        self.tx.is_empty()
    }
}

/// Receiver end of a WsBroadcaster subscription.
pub struct WsBroadcastReceiver {
    rx: broadcast::Receiver<BroadcastMessage>,
    client_id: u64,
}

impl WsBroadcastReceiver {
    /// Returns this receiver's unique client ID (for use with `send_*_from`).
    pub fn client_id(&self) -> u64 {
        self.client_id
    }

    /// Receive the next broadcast message, skipping messages sent by this client.
    ///
    /// Returns the message as an `Arc<Message>` — the broadcaster already keeps
    /// each payload in an `Arc`, so this hands out a cheap clone of the pointer
    /// rather than cloning the full frame bytes. Call `(*msg).clone()` if you
    /// need an owned `Message`.
    pub async fn recv(&mut self) -> Option<Arc<Message>> {
        loop {
            match self.rx.recv().await {
                Ok(msg) => {
                    if msg.sender_id == Some(self.client_id) {
                        continue; // skip own messages
                    }
                    return Some(msg.data);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }
}

// ── WsRooms ──────────────────────────────────────────────────────────────

/// Keyed manager for per-resource WebSocket broadcasters.
///
/// Defaults `K = String` for the common "named chat room" case; parameterize
/// over the key type for typed identifiers (`Uuid`, `UserId`, …). Mirrors
/// [`crate::sse::SseRooms`].
///
/// Clone + Send + Sync (provided `K` is `Send + Sync`) — injectable via
/// `#[inject]`.
#[derive(Clone)]
pub struct WsRooms<K = String>
where
    K: Eq + Hash,
{
    rooms: Arc<DashMap<K, WsBroadcaster>>,
    capacity: usize,
}

impl<K> WsRooms<K>
where
    K: Eq + Hash,
{
    /// Create a new room manager with the given per-room channel capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            rooms: Arc::new(DashMap::new()),
            capacity,
        }
    }

    /// Get or create a broadcaster for the given key.
    pub fn room(&self, key: K) -> WsBroadcaster {
        self.rooms
            .entry(key)
            .or_insert_with(|| WsBroadcaster::new(self.capacity))
            .clone()
    }

    /// Remove and drop the broadcaster for `key`, if any.
    pub fn remove<Q>(&self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.rooms.remove(key);
    }

    /// Drop rooms whose broadcaster has no active subscribers. Call
    /// periodically (or at the end of a workflow) to avoid unbounded
    /// growth when callers forget to `remove(key)` on completion.
    ///
    /// Returns the number of rooms removed.
    pub fn reap_empty(&self) -> usize {
        let before = self.rooms.len();
        self.rooms
            .retain(|_k, broadcaster| broadcaster.subscriber_count() > 0);
        before - self.rooms.len()
    }

    /// Returns the number of active rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Returns true if there are no active rooms.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}
