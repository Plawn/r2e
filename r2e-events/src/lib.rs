//! In-process and distributed event bus for R2E.
//!
//! `LocalEventBus` dispatches in-process by `TypeId` (no serialization, no
//! delivery guarantee across restarts). Distributed backends (Iggy, Kafka,
//! Pulsar, RabbitMQ) live in `r2e-events/backends/` and share the utilities
//! in [`backend`].
//!
//! # Messaging models
//!
//! - **`emit`** — fan-out publish/subscribe. Every subscriber receives a copy;
//!   the emitter does not wait for handlers and cannot observe a reply
//!   (Vert.x `publish` semantics).
//! - **`request` / `respond`** — point-to-point request-reply. Exactly one
//!   responder replies; the requester awaits that reply with a timeout
//!   (Vert.x `request` semantics). At most one responder may be registered per
//!   request type per process; cross-instance load balancing comes from the
//!   broker's queue/consumer-group semantics, not in-process round-robin.
//!
//! # Delivery semantics (distributed backends)
//!
//! **At-least-once.** The broker copy of a message is acked/committed only
//! after every local handler has resolved
//! ([`backend::BackendState::dispatch_from_poller_tracked`]). Handlers must
//! therefore be **idempotent**: redelivery is expected after a crash, a
//! disconnect, or a [`HandlerResult::Nack`]. A `Nack` whose payload was
//! captured to a configured dead-letter topic counts as processed and is
//! acked; a payload that fails to deserialize (poison message) is parked in
//! the matching handlers' configured dead-letter topics (when any) and then
//! acked — never redelivered forever; a panicking handler counts as a
//! `Nack`. There is a single delivery path per consumer group.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub mod backend;
mod local;
pub mod sse_bridge;

pub use local::{LocalEventBus, DEFAULT_MAX_CONCURRENCY};
pub use sse_bridge::SseBridgeExt;

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
    /// `request` found no responder registered for the request type.
    NoResponder,
    /// `request` did not receive a reply within the configured timeout.
    RequestTimeout,
    /// The responder handled the request but returned an error payload
    /// (Vert.x `ReplyException` equivalent).
    Remote(String),
    /// Any other error.
    Other(String),
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Connection(msg) => write!(f, "connection error: {msg}"),
            Self::Shutdown => write!(f, "event bus is shut down"),
            Self::NoResponder => write!(f, "no responder registered for request type"),
            Self::RequestTimeout => write!(f, "request timed out waiting for a reply"),
            Self::Remote(msg) => write!(f, "responder returned an error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for EventBusError {}

// ── EmitReceipt ───────────────────────────────────────────────────────

/// Handle returned by [`EventBus::emit_nowait`] / [`EventBus::emit_nowait_with`].
///
/// Wraps the broker confirmation as a future. The caller can:
/// - Drop it for fire-and-forget (the message is already in-flight).
/// - Call [`confirm`](EmitReceipt::confirm) to await the broker acknowledgement.
/// - Collect many receipts and `try_join_all(receipts.into_iter().map(|r| r.confirm()))`
///   for batch confirmation.
pub struct EmitReceipt {
    inner: Pin<Box<dyn Future<Output = Result<(), EventBusError>> + Send>>,
}

impl EmitReceipt {
    /// Create a receipt from a future that resolves when the broker confirms.
    pub fn new(fut: impl Future<Output = Result<(), EventBusError>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(fut),
        }
    }

    /// Create an already-resolved receipt (for `LocalEventBus` or fallback).
    pub fn ready() -> Self {
        Self {
            inner: Box::pin(std::future::ready(Ok(()))),
        }
    }

    /// Await broker confirmation. Equivalent to using [`EventBus::emit`] directly.
    pub async fn confirm(self) -> Result<(), EventBusError> {
        self.inner.await
    }
}

impl fmt::Debug for EmitReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmitReceipt").finish_non_exhaustive()
    }
}

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

// ── ResponderHandle ────────────────────────────────────────────────────

/// Handle returned by [`EventBus::respond`]. Can be used to unregister the
/// responder so that a different one may take its place.
#[derive(Clone)]
pub struct ResponderHandle {
    type_name: &'static str,
    _unregister: Arc<dyn Fn() + Send + Sync>,
}

impl ResponderHandle {
    /// Create a new handle for the responder of request type named `type_name`,
    /// with the given unregister closure.
    pub fn new(type_name: &'static str, unregister: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            type_name,
            _unregister: Arc::new(unregister),
        }
    }

    /// Remove this responder from the event bus.
    pub fn unregister(&self) {
        (self._unregister)();
    }
}

impl fmt::Debug for ResponderHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponderHandle")
            .field("type_name", &self.type_name)
            .finish()
    }
}

// ── RequestOptions ─────────────────────────────────────────────────────

/// Default timeout applied by [`EventBus::request`].
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Options controlling a single [`EventBus::request_with`] call.
#[derive(Debug, Clone)]
pub struct RequestOptions {
    /// How long to wait for a reply before failing with
    /// [`EventBusError::RequestTimeout`].
    pub timeout: Duration,
    /// Explicit metadata for the request message. When `None`, fresh metadata
    /// is generated.
    pub metadata: Option<EventMetadata>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            metadata: None,
        }
    }
}

impl RequestOptions {
    /// Options with the default 30s timeout and no explicit metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the reply timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set explicit request metadata.
    pub fn with_metadata(mut self, metadata: EventMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

// ── EventMetadata ──────────────────────────────────────────────────────

/// Per-process random identity, occupying the high 64 bits of every
/// `event_id`. Generated once, lazily, on first event emission.
static PROCESS_ID: OnceLock<u64> = OnceLock::new();

/// Per-process monotonic counter, occupying the low 64 bits of `event_id`.
static EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Draw a random 64-bit value from the OS CSPRNG (via `uuid`, already a
/// workspace dependency of the framework).
fn generate_process_id() -> u64 {
    uuid::Uuid::new_v4().as_u64_pair().0
}

/// The per-process identity that prefixes every `event_id` on this instance.
fn process_id() -> u64 {
    *PROCESS_ID.get_or_init(generate_process_id)
}

/// Compose a `u128` event id from a 64-bit process identity (high bits) and a
/// 64-bit counter (low bits). Exposed for tests that need to assert
/// cross-process uniqueness with deterministic inputs.
#[doc(hidden)]
pub fn compose_event_id(process_id: u64, counter: u64) -> u128 {
    ((process_id as u128) << 64) | (counter as u128)
}

/// Generate the next globally-unique event id for this process.
fn next_event_id() -> u128 {
    compose_event_id(process_id(), EVENT_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Metadata attached to every emitted event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Globally-unique event identifier (auto-generated): a `u128` whose high
    /// 64 bits are a per-process random identity and low 64 bits a per-process
    /// counter, so ids never collide across instances of the same app.
    pub event_id: u128,
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
            event_id: next_event_id(),
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

// ── EventFilter ───────────────────────────────────────────────────────

/// A predicate that decides whether a handler should process a given event.
///
/// Receives the event's metadata and returns `true` to process, `false` to skip.
pub type EventFilter = Arc<dyn Fn(&EventMetadata) -> bool + Send + Sync>;

// ── RetryPolicy ───────────────────────────────────────────────────────

/// Configuration for automatic retry and dead-letter handling.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (default: 3).
    pub max_retries: u32,
    /// Delay between retries (default: 1 second).
    pub retry_delay: std::time::Duration,
    /// Whether to use exponential backoff (default: true).
    pub exponential_backoff: bool,
    /// Optional dead-letter topic name. Events that exhaust all retries are
    /// published here. If `None`, failed events are dropped.
    pub dead_letter_topic: Option<String>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay: std::time::Duration::from_secs(1),
            exponential_backoff: true,
            dead_letter_topic: None,
        }
    }
}

impl RetryPolicy {
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    pub fn with_dlq(mut self, topic: impl Into<String>) -> Self {
        self.dead_letter_topic = Some(topic.into());
        self
    }
}

// ── DlqPublisher ──────────────────────────────────────────────────────

/// Callback for publishing failed events to a dead-letter topic.
pub type DlqPublisher = Arc<
    dyn Fn(String, Vec<u8>, EventMetadata) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

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
    /// Register a topic name for event type `E` at runtime.
    ///
    /// Distributed backends write to their topic registry; `LocalEventBus` is a no-op.
    /// This is used by the `#[consumer(topic = "...")]` macro attribute.
    fn register_topic<E: 'static>(&self, _topic: &str) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Configure filter and retry policy on a handler after subscription.
    ///
    /// This is called by generated code to attach filter/retry policies to
    /// a handler identified by its subscription ID. The type parameter `E`
    /// enables O(1) lookup by TypeId instead of scanning all handler maps.
    fn configure_handler<E: 'static>(
        &self,
        _handler_id: SubscriptionId,
        _filter: Option<EventFilter>,
        _retry_policy: Option<RetryPolicy>,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }

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

    /// Subscribe with a custom deserializer for non-JSON formats.
    ///
    /// Use this when consuming events from external systems that use Protobuf,
    /// Avro, MessagePack, or other binary formats. The `deserializer` converts
    /// raw bytes into `Arc<dyn Any + Send + Sync>`.
    ///
    /// Note: `E` does NOT need to implement `DeserializeOwned` when using this method.
    fn subscribe_with_deserializer<E, F, Fut>(
        &self,
        _deserializer: backend::DeserializerFn,
        _handler: F,
    ) -> impl Future<Output = Result<SubscriptionHandle, EventBusError>> + Send
    where
        E: Send + Sync + 'static,
        F: Fn(EventEnvelope<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        async { Err(EventBusError::Other("subscribe_with_deserializer not supported by this backend".to_string())) }
    }

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

    /// Emit an event without waiting for the broker acknowledgement.
    ///
    /// Returns an [`EmitReceipt`] that the caller can:
    /// - Drop for fire-and-forget (the message is already in-flight).
    /// - [`confirm`](EmitReceipt::confirm) to await the broker ack.
    /// - Collect and batch-confirm via `try_join_all`.
    ///
    /// Errors returned by the outer `Result` are pre-flight failures
    /// (serialization, shutdown, queue-full). Broker-level errors surface
    /// through [`EmitReceipt::confirm`].
    ///
    /// The default implementation delegates to [`emit`](EventBus::emit)
    /// (blocking until the broker confirms) and returns a ready receipt —
    /// backends override for true non-blocking behavior.
    fn emit_nowait<E>(
        &self,
        event: E,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let this = self.clone();
        async move {
            this.emit(event).await?;
            Ok(EmitReceipt::ready())
        }
    }

    /// Emit an event with explicit metadata without waiting for the broker
    /// acknowledgement.
    ///
    /// See [`emit_nowait`](EventBus::emit_nowait) for semantics.
    fn emit_nowait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let this = self.clone();
        async move {
            this.emit_with(event, metadata).await?;
            Ok(EmitReceipt::ready())
        }
    }

    /// Send a point-to-point request and await the responder's reply.
    ///
    /// Exactly one responder (registered via [`respond`]) handles the request
    /// and produces the reply. Uses [`DEFAULT_REQUEST_TIMEOUT`]. Errors:
    /// [`EventBusError::NoResponder`] when no responder is registered,
    /// [`EventBusError::RequestTimeout`] when no reply arrives in time,
    /// [`EventBusError::Remote`] when the responder returns an error.
    ///
    /// The default implementation reports that the backend does not support
    /// request-reply — backends that do override it.
    ///
    /// [`respond`]: EventBus::respond
    fn request<Req, Resp>(
        &self,
        req: Req,
    ) -> impl Future<Output = Result<Resp, EventBusError>> + Send
    where
        Req: Serialize + Send + Sync + 'static,
        Resp: DeserializeOwned + Send + 'static,
    {
        self.request_with(req, RequestOptions::default())
    }

    /// Send a point-to-point request with explicit [`RequestOptions`].
    ///
    /// See [`request`](EventBus::request). The default implementation reports
    /// that the backend does not support request-reply.
    fn request_with<Req, Resp>(
        &self,
        _req: Req,
        _options: RequestOptions,
    ) -> impl Future<Output = Result<Resp, EventBusError>> + Send
    where
        Req: Serialize + Send + Sync + 'static,
        Resp: DeserializeOwned + Send + 'static,
    {
        async { Err(EventBusError::Other("request-reply not supported by this backend".to_string())) }
    }

    /// Register the single responder for request type `Req`.
    ///
    /// The handler receives an [`EventEnvelope<Req>`] and returns
    /// `Result<Resp, String>` — `Ok(resp)` becomes the reply, `Err(msg)`
    /// surfaces to the requester as [`EventBusError::Remote`]. At most one
    /// responder may be registered per request type per process; a second
    /// registration returns an error.
    ///
    /// The default implementation reports that the backend does not support
    /// request-reply — backends that do override it.
    fn respond<Req, Resp, F, Fut>(
        &self,
        _handler: F,
    ) -> impl Future<Output = Result<ResponderHandle, EventBusError>> + Send
    where
        Req: DeserializeOwned + Send + Sync + 'static,
        Resp: Serialize + Send + 'static,
        F: Fn(EventEnvelope<Req>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, String>> + Send + 'static,
    {
        async { Err(EventBusError::Other("request-reply not supported by this backend".to_string())) }
    }

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
    pub use crate::backend::DeserializerFn;
    pub use crate::sse_bridge::SseBridgeExt;
    pub use crate::{
        EmitReceipt, Event, EventBus, EventBusError, EventEnvelope, EventFilter, EventMetadata,
        HandlerResult, LocalEventBus, RequestOptions, ResponderHandle, RetryPolicy,
        SubscriptionHandle, SubscriptionId, DEFAULT_REQUEST_TIMEOUT,
    };
}
