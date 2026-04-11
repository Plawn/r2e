//! SSE (Server-Sent Events) broadcaster for multi-client streaming.
//!
//! # Usage
//!
//! ```ignore
//! use r2e_core::sse::{SseBroadcaster, SseRooms};
//!
//! // App-scoped broadcaster (injectable via #[inject])
//! let broadcaster = SseBroadcaster::new(128);
//!
//! // In a handler — subscribe a client
//! let stream = broadcaster.subscribe();
//! Sse::new(stream).keep_alive(SseKeepAlive::default())
//!
//! // From anywhere — broadcast to all clients
//! broadcaster.send("hello").ok();
//! broadcaster.send_event("update", r#"{"count":42}"#).ok();
//!
//! // Opt in to lag signaling: lagged subscribers receive a synthetic
//! // `event: lagged\ndata: <dropped-count>` line instead of silently
//! // dropping events.
//! let stream = broadcaster.subscribe_lagged("lagged");
//! ```
//!
//! # Per-key rooms
//!
//! [`SseRooms<K>`] is the SSE counterpart to [`crate::ws::WsRooms`]: an
//! injectable `DashMap<K, SseBroadcaster>` with lazy-insert
//! `room(key)` / `subscribe(key)` / `remove(key)` helpers. Use it for
//! per-entity streams (per-run logs, per-user notifications, etc.).
//!
//! ```ignore
//! use r2e_core::sse::SseRooms;
//!
//! #[derive(Clone)] // wrap in BeanState in real code
//! struct LogBus { rooms: SseRooms<String> }
//!
//! let bus = LogBus { rooms: SseRooms::new(256) };
//! bus.rooms.room("run-42".to_string()).send("build started").ok();
//! let stream = bus.rooms.subscribe("run-42".to_string());
//! ```

use std::borrow::Borrow;
use std::convert::Infallible;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::http::response::SseEvent;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

// ── SseBroadcaster ───────────────────────────────────────────────────────

/// Message sent through the broadcast channel.
#[derive(Clone, Debug)]
pub struct SseMessage {
    /// Optional event type name.
    pub event: Option<String>,
    /// Event data payload.
    pub data: String,
}

/// Injectable SSE broadcaster for multi-client streaming.
///
/// Clone + Send + Sync — can be used as an `#[inject]` field on controllers.
#[derive(Clone)]
pub struct SseBroadcaster {
    tx: broadcast::Sender<SseMessage>,
    capacity: usize,
}

impl SseBroadcaster {
    /// Create a new broadcaster with the given broadcast channel capacity.
    ///
    /// Capacity is the number of messages a slow subscriber may fall behind
    /// before older messages start being dropped from its queue. Tune for the
    /// expected burstiness and client consumption rate:
    ///
    /// - `cargo build`-style streams (chatty) → 1024+
    /// - Progress updates, occasional notifications → 64–256
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, capacity }
    }

    /// The broadcast channel capacity fixed at construction.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of queued messages across all subscribers.
    pub fn len(&self) -> usize {
        self.tx.len()
    }

    /// Returns true if the broadcast channel currently has no queued messages.
    pub fn is_empty(&self) -> bool {
        self.tx.is_empty()
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Broadcast a data-only event to all subscribers.
    ///
    /// Returns the number of subscribers that received the message on success,
    /// or `Err` if there are no active subscribers.
    pub fn send(
        &self,
        data: impl Into<String>,
    ) -> Result<usize, broadcast::error::SendError<SseMessage>> {
        self.tx.send(SseMessage {
            event: None,
            data: data.into(),
        })
    }

    /// Broadcast a typed event to all subscribers.
    ///
    /// Returns the number of subscribers that received the message on success,
    /// or `Err` if there are no active subscribers.
    pub fn send_event(
        &self,
        event: &str,
        data: impl Into<String>,
    ) -> Result<usize, broadcast::error::SendError<SseMessage>> {
        self.tx.send(SseMessage {
            event: Some(event.to_string()),
            data: data.into(),
        })
    }

    /// Subscribe to events using the given [`LagPolicy`].
    ///
    /// `subscribe()` and `subscribe_lagged()` are ergonomic wrappers around
    /// this method.
    pub fn subscribe_with(&self, policy: LagPolicy) -> SseSubscription {
        SseSubscription::new(self.tx.subscribe(), policy)
    }

    /// Subscribe to events. Lagged messages are silently skipped
    /// (equivalent to `subscribe_with(LagPolicy::Silent)`).
    ///
    /// Use [`subscribe_lagged`](Self::subscribe_lagged) if you need the client
    /// (and/or the server loop consuming the stream) to observe that drops
    /// occurred.
    pub fn subscribe(&self) -> SseSubscription {
        self.subscribe_with(LagPolicy::Silent)
    }

    /// Subscribe to events, emitting a synthetic SSE event of the given type
    /// whenever the subscriber's receive queue lags behind the sender. The
    /// synthetic event's `data` is the number of messages that were dropped
    /// as a decimal string.
    ///
    /// Equivalent to `subscribe_with(LagPolicy::Synthetic(event_name.into()))`.
    /// This lets SSE clients detect — and recover from — dropped messages
    /// (for example, by re-fetching state from a REST endpoint).
    pub fn subscribe_lagged(&self, event_name: impl Into<String>) -> SseSubscription {
        self.subscribe_with(LagPolicy::Synthetic(event_name.into()))
    }
}

// ── LagPolicy ────────────────────────────────────────────────────────────

/// How an [`SseSubscription`] should react when its receive queue lags
/// behind the broadcaster.
///
/// Tokio's broadcast channel drops the oldest messages once a slow
/// subscriber falls behind by more than the channel capacity. `LagPolicy`
/// chooses whether that drop is observable on the stream.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum LagPolicy {
    /// Silently skip dropped messages. The subscriber simply resumes with
    /// the next live message.
    Silent,
    /// Emit a synthetic SSE event of the given type whenever messages are
    /// dropped. The event's `data` field is the number of dropped messages
    /// as a decimal string.
    Synthetic(String),
}

fn msg_to_event(msg: SseMessage) -> SseEvent {
    let mut event = SseEvent::default().data(msg.data);
    if let Some(ref name) = msg.event {
        event = event.event(name);
    }
    event
}

/// A subscription stream that yields SSE events.
///
/// Implements `Stream<Item = Result<SseEvent, Infallible>>` — ready to pass
/// to `Sse::new()`.
///
/// Internally backed by [`tokio_stream::wrappers::BroadcastStream`], which
/// uses a single reusable boxed future — there is no per-poll allocation
/// on the hot path.
pub struct SseSubscription {
    inner: BroadcastStream<SseMessage>,
    policy: LagPolicy,
}

impl SseSubscription {
    fn new(rx: broadcast::Receiver<SseMessage>, policy: LagPolicy) -> Self {
        Self {
            inner: BroadcastStream::new(rx),
            policy,
        }
    }
}

impl futures_core::Stream for SseSubscription {
    type Item = Result<SseEvent, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(msg))) => return Poll::Ready(Some(Ok(msg_to_event(msg)))),
                Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(n)))) => match &this.policy {
                    LagPolicy::Synthetic(name) => {
                        let event = SseEvent::default().event(name.as_str()).data(n.to_string());
                        return Poll::Ready(Some(Ok(event)));
                    }
                    LagPolicy::Silent => continue,
                },
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ── SseRooms ─────────────────────────────────────────────────────────────

/// Keyed manager for per-resource SSE broadcasters.
///
/// `SseRooms<K>` is the SSE counterpart of [`crate::ws::WsRooms`]: a
/// `DashMap<K, SseBroadcaster>` with lazy-insert helpers. Use it when an
/// application needs a separate stream per entity — e.g. per-run logs,
/// per-user notifications, per-tenant event feeds.
///
/// Clone + Send + Sync (provided `K` is `Send + Sync`) — injectable via
/// `#[inject]`.
///
/// Unlike `WsRooms`, `SseRooms` is generic over the key type so callers
/// can use typed identifiers (`Uuid`, `RunId`, `(TenantId, UserId)`, …)
/// without stringifying.
#[derive(Clone)]
pub struct SseRooms<K>
where
    K: Eq + Hash,
{
    rooms: Arc<DashMap<K, SseBroadcaster>>,
    capacity: usize,
}

impl<K> SseRooms<K>
where
    K: Eq + Hash,
{
    /// Create a new room manager with the given per-room broadcast channel
    /// capacity. Every broadcaster created via [`room`](Self::room) or
    /// [`subscribe`](Self::subscribe) will use this capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            rooms: Arc::new(DashMap::new()),
            capacity,
        }
    }

    /// Get or create the broadcaster for `key`.
    ///
    /// The returned broadcaster is cheap to clone — it's backed by the same
    /// Tokio channel as any other handle for the same key.
    pub fn room(&self, key: K) -> SseBroadcaster {
        self.rooms
            .entry(key)
            .or_insert_with(|| SseBroadcaster::new(self.capacity))
            .clone()
    }

    /// Shorthand for `self.room(key).subscribe_with(policy)`.
    ///
    /// Creates the room if it does not yet exist.
    pub fn subscribe_with(&self, key: K, policy: LagPolicy) -> SseSubscription {
        self.room(key).subscribe_with(policy)
    }

    /// Shorthand for `self.room(key).subscribe()`.
    ///
    /// Creates the room if it does not yet exist.
    pub fn subscribe(&self, key: K) -> SseSubscription {
        self.room(key).subscribe()
    }

    /// Shorthand for `self.room(key).subscribe_lagged(event_name)`.
    pub fn subscribe_lagged(&self, key: K, event_name: impl Into<String>) -> SseSubscription {
        self.room(key).subscribe_lagged(event_name)
    }

    /// Remove and drop the broadcaster for `key`, if any.
    ///
    /// Active subscribers keep receiving messages sent prior to the removal,
    /// then see the stream end once the sender handle is dropped.
    pub fn remove<Q>(&self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.rooms.remove(key);
    }

    /// Drop rooms whose broadcaster has no active subscribers. Call
    /// periodically (or at the end of a workflow) to avoid unbounded growth
    /// when callers forget to `remove(key)` on completion.
    ///
    /// Returns the number of rooms removed.
    pub fn reap_empty(&self) -> usize {
        let before = self.rooms.len();
        self.rooms
            .retain(|_k, broadcaster| broadcaster.subscriber_count() > 0);
        before - self.rooms.len()
    }

    /// Returns the number of live rooms (keys with an active broadcaster).
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Returns true if there are no live rooms.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}
