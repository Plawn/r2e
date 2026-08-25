//! WebSocket utilities — ergonomic wrappers, handler trait, and broadcaster.
//!
//! # WsStream
//!
//! An ergonomic wrapper around the underlying [`WebSocket`](crate::http::ws::WebSocket)
//! with typed helpers for text, JSON, and binary messages.
//!
//! `send*` methods write **and flush** one frame at a time. For bulk transport,
//! the explicit batching API (`feed*` + `flush`) lets a caller queue several
//! frames and hand them to the socket in a single write — see
//! [`WsStream::feed`].
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
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::http::ws::{Message, WebSocket};
use crate::rt::sync::broadcast;
use dashmap::DashMap;
use futures_util::{Sink, SinkExt};
use serde::{de::DeserializeOwned, Serialize};

// ── WsError ──────────────────────────────────────────────────────────────

/// Errors from WebSocket operations.
#[derive(Debug)]
pub enum WsError {
    Send(crate::http::Error),
    Recv(crate::http::Error),
    Json(crate::json::JsonError),
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

/// Ergonomic wrapper around the underlying WebSocket with typed helpers.
///
/// # Sending: `send*` vs `feed*` + `flush`
///
/// Every `send*` method (`send`, `send_text`, `send_json`, `send_binary`)
/// queues **one** frame and flushes it to the socket immediately — the message
/// is on the wire when the future resolves. That is the right default for
/// request/response style traffic.
///
/// For bulk transport (proxies, file streaming, fan-out of many small frames)
/// use the explicit batching API instead: [`feed`](Self::feed) /
/// [`feed_text`](Self::feed_text) / [`feed_binary`](Self::feed_binary) queue a
/// frame **without** flushing, and one [`flush`](Self::flush) pushes everything
/// queued so far in a single write. Fewer syscalls, fewer wakeups, same frame
/// order. There is no timer and no background writer: nothing leaves the
/// process until you call `flush` (or a `send*`, which flushes too).
///
/// ```ignore
/// for message in batch {
///     ws.feed(message).await?;
/// }
/// ws.flush().await?;
/// ```
///
/// `feed*` still awaits the sink's readiness before queueing, so backpressure
/// from the transport is honoured. The framework does **not** cap what you
/// queue between flushes — the caller must bound the batch (number of frames
/// and total bytes, e.g. 16 frames / 256 KiB) to keep memory and latency in
/// check.
///
/// `WsStream` also implements [`futures_util::Sink<Message>`], so it composes
/// with `SinkExt` combinators (`send_all`, `forward`, …).
pub struct WsStream {
    inner: WebSocket,
}

impl crate::http::ws::IsWebSocket for WsStream {}

impl WsStream {
    /// Wrap a raw Axum WebSocket.
    pub fn new(socket: WebSocket) -> Self {
        Self { inner: socket }
    }

    // ── Send (queue + flush) ──

    /// Send a raw message: queue it and flush the socket.
    ///
    /// The frame is on the wire when this resolves. Equivalent to
    /// [`feed`](Self::feed) followed by [`flush`](Self::flush).
    pub async fn send(&mut self, msg: Message) -> Result<(), WsError> {
        self.inner.send(msg).await.map_err(WsError::Send)
    }

    /// Send a text message.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), WsError> {
        self.send(Message::Text(text.into().into())).await
    }

    /// Send a JSON-serialized message.
    pub async fn send_json<T: Serialize>(&mut self, data: &T) -> Result<(), WsError> {
        let json = crate::json::to_string(data).map_err(WsError::Json)?;
        self.send_text(json).await
    }

    /// Send a binary message.
    pub async fn send_binary(&mut self, data: impl Into<bytes::Bytes>) -> Result<(), WsError> {
        self.send(Message::Binary(data.into())).await
    }

    // ── Batching (queue without flush) ──

    /// Queue a raw message **without** flushing.
    ///
    /// Waits for the sink to be ready (backpressure), then enqueues the frame.
    /// Nothing is written to the socket until [`flush`](Self::flush) — or any
    /// `send*` call, which flushes as part of its contract. Frames are
    /// delivered in `feed` order.
    ///
    /// Bound the batch yourself: there is no built-in cap on queued frames or
    /// bytes between flushes.
    ///
    /// ```ignore
    /// for message in batch {
    ///     ws.feed(message).await?;
    /// }
    /// ws.flush().await?;
    /// ```
    pub async fn feed(&mut self, msg: Message) -> Result<(), WsError> {
        self.inner.feed(msg).await.map_err(WsError::Send)
    }

    /// Queue a text message without flushing. See [`feed`](Self::feed).
    pub async fn feed_text(&mut self, text: impl Into<String>) -> Result<(), WsError> {
        self.feed(Message::Text(text.into().into())).await
    }

    /// Queue a binary message without flushing. See [`feed`](Self::feed).
    ///
    /// The payload is moved into the frame as [`bytes::Bytes`] — no copy.
    pub async fn feed_binary(&mut self, data: impl Into<bytes::Bytes>) -> Result<(), WsError> {
        self.feed(Message::Binary(data.into())).await
    }

    /// Flush every queued frame to the socket.
    ///
    /// Valid with nothing queued (no-op). Resolves once the transport has
    /// accepted all pending frames.
    pub async fn flush(&mut self) -> Result<(), WsError> {
        self.inner.flush().await.map_err(WsError::Send)
    }

    /// Flush queued frames, then close the sink.
    ///
    /// Sends the WebSocket close handshake; further `send*`/`feed*` calls fail.
    pub async fn close(&mut self) -> Result<(), WsError> {
        SinkExt::close(&mut self.inner).await.map_err(WsError::Send)
    }

    // ── Receive ──

    /// Receive the next message, or `None` if the connection is closed.
    pub async fn next(&mut self) -> Option<Result<Message, WsError>> {
        use crate::rt::stream::StreamExt;
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
                    return Some(crate::json::from_slice(bytes.as_bytes()).map_err(WsError::Json));
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

/// `Sink<Message>` bridge: `poll_ready`/`start_send` queue, `poll_flush`
/// writes, `poll_close` flushes then closes — the same semantics as the
/// inherent `feed`/`flush`/`close`, exposed for `SinkExt` combinators.
impl Sink<Message> for WsStream {
    type Error = WsError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.inner)
            .poll_ready(cx)
            .map_err(WsError::Send)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), WsError> {
        Pin::new(&mut self.inner)
            .start_send(item)
            .map_err(WsError::Send)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.inner)
            .poll_flush(cx)
            .map_err(WsError::Send)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.inner)
            .poll_close(cx)
            .map_err(WsError::Send)
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
    pub fn send_json<T: Serialize>(&self, data: &T) -> Result<(), crate::json::JsonError> {
        let json = crate::json::to_string(data)?;
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
    ) -> Result<(), crate::json::JsonError> {
        let json = crate::json::to_string(data)?;
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
    /// Matches [`broadcast::Sender::is_empty`] — reflects the slowest
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
/// [`crate::web::sse::SseRooms`].
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
