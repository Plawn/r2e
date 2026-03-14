use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

mod local;

pub use local::{LocalEventBus, DEFAULT_MAX_CONCURRENCY};

// ── EventBusError ──────────────────────────────────────────────────────

/// Errors that can occur in event bus operations.
#[derive(Debug, Clone)]
pub enum EventBusError {
    /// Serialization or deserialization failure.
    Serialization(String),
    /// Connection failure (relevant for distributed backends).
    Connection(String),
    /// The event bus has been shut down.
    Shutdown,
    /// Any other error.
    Other(String),
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Connection(msg) => write!(f, "connection error: {msg}"),
            Self::Shutdown => write!(f, "event bus is shut down"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for EventBusError {}

// ── SubscriptionHandle ─────────────────────────────────────────────────

/// Opaque identifier for a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Handle returned by [`EventBus::subscribe`]. Can be used to unsubscribe.
#[derive(Clone)]
pub struct SubscriptionHandle {
    id: SubscriptionId,
    _unsubscribe: Arc<dyn Fn() + Send + Sync>,
}

impl SubscriptionHandle {
    /// Create a new handle with the given id and unsubscribe closure.
    pub fn new(id: SubscriptionId, unsubscribe: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            id,
            _unsubscribe: Arc::new(unsubscribe),
        }
    }

    /// Remove this subscription from the event bus.
    pub fn unsubscribe(&self) {
        (self._unsubscribe)();
    }

    /// Returns the subscription id.
    pub fn id(&self) -> SubscriptionId {
        self.id
    }
}

impl fmt::Debug for SubscriptionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("id", &self.id)
            .finish()
    }
}

// ── EventMetadata ──────────────────────────────────────────────────────

static GLOBAL_EVENT_ID: AtomicU64 = AtomicU64::new(1);

/// Metadata attached to every emitted event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Unique event identifier (auto-generated).
    pub event_id: u64,
    /// Optional correlation id for tracing across services.
    pub correlation_id: Option<String>,
    /// Timestamp in epoch milliseconds.
    pub timestamp: u64,
    /// Optional partition key for distributed backends.
    pub partition_key: Option<String>,
    /// Arbitrary key-value headers.
    pub headers: HashMap<String, String>,
}

impl EventMetadata {
    /// Create new metadata with auto-generated event_id and current timestamp.
    pub fn new() -> Self {
        Self {
            event_id: GLOBAL_EVENT_ID.fetch_add(1, Ordering::Relaxed),
            correlation_id: None,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            partition_key: None,
            headers: HashMap::new(),
        }
    }

    /// Set a correlation id.
    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    /// Set a partition key.
    pub fn with_partition_key(mut self, key: impl Into<String>) -> Self {
        self.partition_key = Some(key.into());
        self
    }

    /// Add a header.
    pub fn with_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.insert(k.into(), v.into());
        self
    }
}

impl Default for EventMetadata {
    fn default() -> Self {
        Self::new()
    }
}

// ── EventEnvelope ──────────────────────────────────────────────────────

/// Wraps an event with its metadata.
#[derive(Debug, Clone)]
pub struct EventEnvelope<E> {
    /// The event payload.
    pub event: Arc<E>,
    /// Associated metadata.
    pub metadata: EventMetadata,
}

// ── HandlerResult ──────────────────────────────────────────────────────

/// Result returned by event handlers for ack/nack semantics.
#[derive(Debug)]
pub enum HandlerResult {
    /// Handler processed the event successfully.
    Ack,
    /// Handler failed to process the event.
    Nack(String),
}

impl From<()> for HandlerResult {
    fn from(_: ()) -> Self {
        HandlerResult::Ack
    }
}

impl<E: fmt::Display> From<Result<(), E>> for HandlerResult {
    fn from(result: Result<(), E>) -> Self {
        match result {
            Ok(()) => HandlerResult::Ack,
            Err(e) => HandlerResult::Nack(e.to_string()),
        }
    }
}

// ── Event trait ────────────────────────────────────────────────────────

/// Opt-in trait for events that need explicit topic names.
///
/// Distributed backends (Kafka, NATS, Iggy) use `Event::topic()` for routing.
/// `LocalEventBus` ignores it (routes by `TypeId`).
///
/// The `EventBus` trait does **not** require `E: Event` — this is opt-in.
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Returns the topic name for this event type.
    fn topic() -> &'static str {
        std::any::type_name::<Self>()
    }
}

// ── EventBus trait ─────────────────────────────────────────────────────

/// Pluggable event bus trait — implement for custom backends (Kafka, Redis, etc.).
///
/// All methods are generic and monomorphized at compile time. No dynamic
/// dispatch. Serde bounds are enforced so backends can serialize events
/// for remote transport. In-memory backends (like [`LocalEventBus`]) are
/// free to ignore serialization — the bounds exist only for trait compatibility.
pub trait EventBus: Clone + Send + Sync + 'static {
    /// Subscribe to events of type `E`.
    ///
    /// The handler receives an [`EventEnvelope<E>`] and returns a [`HandlerResult`].
    /// Returns a [`SubscriptionHandle`] that can be used to unsubscribe.
    fn subscribe<E, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<SubscriptionHandle, EventBusError>> + Send
    where
        E: DeserializeOwned + Send + Sync + 'static,
        F: Fn(EventEnvelope<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static;

    /// Emit an event, spawning all subscribers as concurrent tasks.
    ///
    /// Returns after all handlers have been spawned (not necessarily completed).
    /// Metadata is auto-generated.
    fn emit<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static;

    /// Emit an event with explicit metadata.
    fn emit_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static;

    /// Emit an event and wait for all subscribers to complete.
    /// Metadata is auto-generated.
    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static;

    /// Emit an event and wait for all subscribers to complete, with explicit metadata.
    fn emit_and_wait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static;

    /// Remove all registered event handlers.
    fn clear(&self) -> impl Future<Output = ()> + Send;

    /// Gracefully shut down the event bus.
    ///
    /// Sets a shutdown flag (new emits will return `Err(Shutdown)`),
    /// then waits up to `timeout` for in-flight handlers to complete.
    fn shutdown(
        &self,
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send;
}

pub mod prelude {
    //! Re-exports of the most commonly used event types.
    pub use crate::{
        Event, EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult,
        LocalEventBus, SubscriptionHandle, SubscriptionId,
    };
}
