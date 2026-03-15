use std::any::TypeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{DlqPublisher, EventBusError, EventMetadata, HandlerResult, SubscriptionHandle, SubscriptionId};
use super::dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
use super::topic::TopicRegistry;

/// Bounded set of event IDs that were dispatched locally by `emit_and_wait`.
///
/// Used to prevent double-delivery: when a distributed backend publishes to
/// the broker AND calls `dispatch_local()`, the background poller would also
/// receive the same message and dispatch again. This set records which event
/// IDs were already handled locally so the poller can skip them.
///
/// Memory is constant: when the set reaches `capacity`, the oldest entry is
/// evicted before inserting the new one. Evictions are logged as warnings and
/// counted via `eviction_count`.
pub struct LocallyDispatchedSet {
    set: HashSet<u64>,
    order: VecDeque<u64>,
    capacity: usize,
    /// Total number of entries evicted because the set was full.
    eviction_count: u64,
}

impl LocallyDispatchedSet {
    pub fn new(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
            eviction_count: 0,
        }
    }

    /// Record an event ID as locally dispatched.
    pub fn insert(&mut self, id: u64) {
        if self.set.contains(&id) {
            return;
        }
        // Drain leading ghost entries (already removed from set by the poller)
        // to prevent unbounded VecDeque growth.
        while self.order.front().is_some_and(|&front| !self.set.contains(&front)) {
            self.order.pop_front();
        }
        if self.set.len() >= self.capacity {
            // Evict oldest live entry; skip any remaining ghosts.
            loop {
                match self.order.pop_front() {
                    Some(oldest) if self.set.remove(&oldest) => {
                        self.eviction_count += 1;
                        tracing::warn!(
                            capacity = self.capacity,
                            eviction_count = self.eviction_count,
                            "dedup set at capacity — evicting oldest entry; \
                             this may cause double-delivery if the poller hasn't processed it yet"
                        );
                        break;
                    }
                    Some(_) => continue, // ghost entry, skip
                    None => break,       // deque empty (shouldn't happen)
                }
            }
        }
        self.set.insert(id);
        self.order.push_back(id);
    }

    /// Remove an event ID, returning `true` if it was present (i.e. already dispatched locally).
    pub fn remove(&mut self, id: u64) -> bool {
        self.set.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Total number of entries evicted due to capacity pressure.
    pub fn eviction_count(&self) -> u64 {
        self.eviction_count
    }
}

/// Shared inner state for distributed event bus backends.
///
/// Contains all fields that are common across backends (Iggy, Kafka,
/// Pulsar, RabbitMQ). Backend-specific state (clients, producers) is
/// stored alongside this struct in the backend's own inner type.
pub struct BackendState {
    pub shutdown: AtomicBool,
    pub next_id: AtomicU64,
    /// Per-TypeId handler registry.
    pub handlers: RwLock<HashMap<TypeId, TopicHandlers>>,
    /// TypeId -> resolved topic name.
    pub topic_registry: RwLock<TopicRegistry>,
    /// Cancellation tokens for background pollers, keyed by TypeId.
    pub poller_cancels: Mutex<HashMap<TypeId, CancellationToken>>,
    /// Number of handlers currently executing.
    pub in_flight: AtomicUsize,
    /// Notified when `in_flight` drops to zero.
    pub in_flight_zero: Notify,
    /// Set of topics already ensured to exist in the backend.
    pub ensured_topics: Mutex<HashSet<String>>,
    /// Event IDs that were dispatched locally by `emit_and_wait`, used to
    /// prevent double-delivery when the poller receives the same message.
    pub locally_dispatched: Mutex<LocallyDispatchedSet>,
    /// Optional callback for publishing failed events to a dead-letter topic.
    /// Provided by the backend when constructing `BackendState`.
    pub dlq_publisher: Option<DlqPublisher>,
}

impl BackendState {
    /// Create a new `BackendState` with the given topic registry.
    pub fn new(topic_registry: TopicRegistry) -> Self {
        Self::with_dlq_publisher(topic_registry, None)
    }

    /// Create a new `BackendState` with a DLQ publisher callback.
    pub fn with_dlq_publisher(topic_registry: TopicRegistry, dlq_publisher: Option<DlqPublisher>) -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            handlers: RwLock::new(HashMap::new()),
            topic_registry: RwLock::new(topic_registry),
            poller_cancels: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            in_flight_zero: Notify::new(),
            ensured_topics: Mutex::new(HashSet::new()),
            locally_dispatched: Mutex::new(LocallyDispatchedSet::new(8192)),
            dlq_publisher,
        }
    }

    /// Check if the bus is shut down, returning `Err(Shutdown)` if so.
    pub fn check_shutdown(&self) -> Result<(), EventBusError> {
        if self.shutdown.load(Ordering::SeqCst) {
            Err(EventBusError::Shutdown)
        } else {
            Ok(())
        }
    }

    /// Allocate the next handler ID.
    pub fn next_handler_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Resolve the topic name for an event type.
    pub async fn resolve_topic<E: 'static>(&self) -> String {
        let type_id = TypeId::of::<E>();
        let type_name = std::any::type_name::<E>();
        let reg = self.topic_registry.read().await;
        reg.resolve(type_id, type_name)
    }

    /// Check if a topic has already been ensured.
    ///
    /// Returns `true` if the topic was previously marked as ensured.
    pub async fn is_topic_ensured(&self, topic_name: &str) -> bool {
        self.ensured_topics.lock().await.contains(topic_name)
    }

    /// Mark a topic as ensured (call after successful creation).
    pub async fn set_topic_ensured(&self, topic_name: &str) {
        self.ensured_topics.lock().await.insert(topic_name.to_string());
    }

    /// Register a handler for an event type, returning `(handler_id, is_first_for_type)`.
    pub async fn register_handler<E>(
        &self,
        handler: Handler,
    ) -> (u64, bool)
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        let json_deser: DeserializerFn = Arc::new(|bytes: &[u8]| {
            serde_json::from_slice::<E>(bytes)
                .map(|e| Arc::new(e) as Arc<dyn std::any::Any + Send + Sync>)
                .map_err(|e| e.to_string())
        });
        self.register_handler_inner::<E>(handler, json_deser, None, None).await
    }

    /// Register a handler with a custom deserializer.
    pub async fn register_handler_with_deserializer<E>(
        &self,
        handler: Handler,
        deserializer: DeserializerFn,
    ) -> (u64, bool)
    where
        E: Send + Sync + 'static,
    {
        self.register_handler_inner::<E>(handler, deserializer, None, None).await
    }

    /// Register a handler with full configuration (filter, retry, custom deserializer).
    pub async fn register_handler_full<E>(
        &self,
        handler: Handler,
        deserializer: Option<DeserializerFn>,
        filter: Option<crate::EventFilter>,
        retry_policy: Option<crate::RetryPolicy>,
    ) -> (u64, bool)
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        let deser = deserializer.unwrap_or_else(|| {
            Arc::new(|bytes: &[u8]| {
                serde_json::from_slice::<E>(bytes)
                    .map(|e| Arc::new(e) as Arc<dyn std::any::Any + Send + Sync>)
                    .map_err(|e| e.to_string())
            })
        });
        self.register_handler_inner::<E>(handler, deser, filter, retry_policy).await
    }

    async fn register_handler_inner<E: 'static>(
        &self,
        handler: Handler,
        deserializer: DeserializerFn,
        filter: Option<crate::EventFilter>,
        retry_policy: Option<crate::RetryPolicy>,
    ) -> (u64, bool) {
        let type_id = TypeId::of::<E>();
        let id = self.next_handler_id();

        let mut map = self.handlers.write().await;
        let is_first = !map.contains_key(&type_id);

        let topic_entry = map.entry(type_id).or_insert_with(|| {
            TopicHandlers {
                entries: Vec::new(),
                deserializer,
            }
        });

        topic_entry.entries.push(HandlerEntry {
            id,
            handler,
            filter,
            retry_policy,
        });
        (id, is_first)
    }

    /// Build an unsubscribe handle for a handler.
    pub fn build_unsubscribe_handle(
        self: &Arc<Self>,
        type_id: TypeId,
        handler_id: u64,
    ) -> SubscriptionHandle {
        let state = self.clone();
        SubscriptionHandle::new(SubscriptionId(handler_id), move || {
            let state = state.clone();
            tokio::spawn(async move {
                let mut map = state.handlers.write().await;
                if let Some(th) = map.get_mut(&type_id) {
                    th.entries.retain(|e| e.id != handler_id);
                    if th.entries.is_empty() {
                        map.remove(&type_id);
                        let mut cancels = state.poller_cancels.lock().await;
                        if let Some(cancel) = cancels.remove(&type_id) {
                            cancel.cancel();
                        }
                    }
                }
            });
        })
    }

    /// Dispatch a deserialized event to all local handlers for `emit_and_wait`.
    pub async fn dispatch_local(
        &self,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) -> Result<(), EventBusError> {
        // Record this event ID so the poller skips it (prevents double-delivery).
        self.locally_dispatched.lock().await.insert(metadata.event_id);

        let map = self.handlers.read().await;
        let topic_handlers = match map.get(&type_id) {
            Some(th) => th,
            None => return Ok(()),
        };

        let event = (topic_handlers.deserializer)(payload)
            .map_err(EventBusError::Serialization)?;

        let mut tasks = Vec::new();
        for entry in &topic_handlers.entries {
            // Check filter
            if entry.filter.as_ref().is_some_and(|f| !f(&metadata)) {
                continue;
            }
            let h = entry.handler.clone();
            let e = event.clone();
            let m = metadata.clone();
            tasks.push(tokio::spawn(async move { h(e, m).await }));
        }

        for task in tasks {
            if let Ok(HandlerResult::Nack(reason)) = task.await {
                tracing::warn!("event handler returned Nack: {reason}");
            }
        }

        Ok(())
    }

    /// Dispatch a message from a poller to local handlers (fire-and-forget with in-flight tracking).
    pub async fn dispatch_from_poller(
        self: &Arc<Self>,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) {
        // Skip if this event was already dispatched locally by emit_and_wait.
        if self.locally_dispatched.lock().await.remove(metadata.event_id) {
            return;
        }

        let map = self.handlers.read().await;
        let topic_handlers = match map.get(&type_id) {
            Some(th) => th,
            None => return,
        };

        let event = match (topic_handlers.deserializer)(payload) {
            Ok(e) => e,
            Err(err) => {
                tracing::error!("failed to deserialize event: {err}");
                return;
            }
        };

        // Pre-allocate DLQ data only if any handler has a DLQ configured.
        let has_dlq = self.dlq_publisher.is_some()
            && topic_handlers.entries.iter().any(|e| {
                e.retry_policy.as_ref().and_then(|p| p.dead_letter_topic.as_ref()).is_some()
            });
        let payload_for_dlq: Option<Arc<Vec<u8>>> = if has_dlq {
            Some(Arc::new(payload.to_vec()))
        } else {
            None
        };
        let meta_for_dlq: Option<EventMetadata> = if has_dlq {
            Some(metadata.clone())
        } else {
            None
        };

        for entry in &topic_handlers.entries {
            // Check filter
            if entry.filter.as_ref().is_some_and(|f| !f(&metadata)) {
                continue;
            }

            let h = entry.handler.clone();
            let e = event.clone();
            let m = metadata.clone();
            let retry_policy = entry.retry_policy.clone();

            self.in_flight.fetch_add(1, Ordering::SeqCst);

            let state = self.clone();
            let payload_for_dlq = payload_for_dlq.clone();
            let meta_for_dlq = meta_for_dlq.clone();
            tokio::spawn(async move {
                let result = if let Some(ref policy) = retry_policy {
                    Self::invoke_with_retry(&h, &e, &m, policy).await
                } else {
                    h(e, m).await
                };
                if state.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
                    state.in_flight_zero.notify_waiters();
                }
                if let HandlerResult::Nack(ref reason) = result {
                    tracing::warn!("event handler returned Nack: {reason}");
                    // Publish to DLQ if configured
                    if let Some(ref policy) = retry_policy {
                        if let Some(ref dlq_topic) = policy.dead_letter_topic {
                            if let (Some(ref publisher), Some(ref pl), Some(ref meta)) =
                                (&state.dlq_publisher, &payload_for_dlq, &meta_for_dlq)
                            {
                                publisher(dlq_topic.clone(), pl.as_ref().clone(), meta.clone()).await;
                            }
                        }
                    }
                }
            });
        }
    }

    /// Configure filter and retry policy on an existing handler entry.
    ///
    /// This is called by generated code after `subscribe()` to attach
    /// filter and retry policies to a handler identified by its ID.
    /// If `type_id_hint` is provided, only that type's handlers are searched (O(1) lookup).
    pub async fn configure_handler(
        &self,
        handler_id: u64,
        filter: Option<crate::EventFilter>,
        retry_policy: Option<crate::RetryPolicy>,
        type_id_hint: Option<TypeId>,
    ) {
        let mut map = self.handlers.write().await;
        if let Some(type_id) = type_id_hint {
            if let Some(topic_handlers) = map.get_mut(&type_id) {
                if let Some(entry) = topic_handlers.entries.iter_mut().find(|e| e.id == handler_id) {
                    entry.filter = filter;
                    entry.retry_policy = retry_policy;
                    return;
                }
            }
        }
        // Fallback: scan all types
        for topic_handlers in map.values_mut() {
            if let Some(entry) = topic_handlers.entries.iter_mut().find(|e| e.id == handler_id) {
                entry.filter = filter;
                entry.retry_policy = retry_policy;
                return;
            }
        }
    }

    /// Invoke a handler with retry logic.
    pub async fn invoke_with_retry(
        handler: &Handler,
        event: &Arc<dyn std::any::Any + Send + Sync>,
        metadata: &EventMetadata,
        policy: &crate::RetryPolicy,
    ) -> HandlerResult {
        let mut last_result = handler(event.clone(), metadata.clone()).await;
        if matches!(last_result, HandlerResult::Ack) {
            return last_result;
        }

        for attempt in 0..policy.max_retries {
            let delay = if policy.exponential_backoff {
                policy.retry_delay * 2u32.saturating_pow(attempt)
            } else {
                policy.retry_delay
            };
            tokio::time::sleep(delay).await;

            tracing::debug!(
                attempt = attempt + 1,
                max = policy.max_retries,
                "retrying event handler"
            );

            last_result = handler(event.clone(), metadata.clone()).await;
            if matches!(last_result, HandlerResult::Ack) {
                return last_result;
            }
        }

        last_result
    }

    /// Cancel all background pollers.
    pub async fn cancel_all_pollers(&self) {
        let mut cancels: HashMap<TypeId, CancellationToken> =
            std::mem::take(&mut *self.poller_cancels.lock().await);
        for (_, cancel) in cancels.drain() {
            cancel.cancel();
        }
    }

    /// Wait for all in-flight handlers to complete, with timeout.
    ///
    /// Returns `Ok(())` if all handlers finished, or `Err` if timed out.
    pub async fn wait_in_flight(
        &self,
        timeout: std::time::Duration,
    ) -> Result<(), EventBusError> {
        let wait = async {
            loop {
                // Register the notified future BEFORE checking the counter
                // to avoid a TOCTOU race where the last handler finishes
                // between our load and the notified() call.
                let notified = self.in_flight_zero.notified();
                if self.in_flight.load(Ordering::SeqCst) == 0 {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(timeout, wait).await.is_err() {
            self.handlers.write().await.clear();
            return Err(EventBusError::Other(format!(
                "shutdown timed out with {} handlers still in flight",
                self.in_flight.load(Ordering::SeqCst)
            )));
        }
        Ok(())
    }
}
