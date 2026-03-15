use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{EventBusError, EventMetadata, HandlerResult, SubscriptionHandle, SubscriptionId};
use super::dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
use super::topic::TopicRegistry;

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
}

impl BackendState {
    /// Create a new `BackendState` with the given topic registry.
    pub fn new(topic_registry: TopicRegistry) -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            handlers: RwLock::new(HashMap::new()),
            topic_registry: RwLock::new(topic_registry),
            poller_cancels: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            in_flight_zero: Notify::new(),
            ensured_topics: Mutex::new(HashSet::new()),
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
        let type_id = TypeId::of::<E>();
        let id = self.next_handler_id();

        let mut map = self.handlers.write().await;
        let is_first = !map.contains_key(&type_id);

        let topic_entry = map.entry(type_id).or_insert_with(|| {
            let deser: DeserializerFn = Arc::new(|bytes: &[u8]| {
                serde_json::from_slice::<E>(bytes)
                    .map(|e| Arc::new(e) as Arc<dyn std::any::Any + Send + Sync>)
                    .map_err(|e| e.to_string())
            });
            TopicHandlers {
                entries: Vec::new(),
                deserializer: deser,
            }
        });

        topic_entry.entries.push(HandlerEntry { id, handler });
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
        let map = self.handlers.read().await;
        let topic_handlers = match map.get(&type_id) {
            Some(th) => th,
            None => return Ok(()),
        };

        let event = (topic_handlers.deserializer)(payload)
            .map_err(EventBusError::Serialization)?;

        let mut tasks = Vec::new();
        for entry in &topic_handlers.entries {
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

        for entry in &topic_handlers.entries {
            let h = entry.handler.clone();
            let e = event.clone();
            let m = metadata.clone();

            self.in_flight.fetch_add(1, Ordering::SeqCst);

            let state = self.clone();
            tokio::spawn(async move {
                let result = h(e, m).await;
                if state.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
                    state.in_flight_zero.notify_waiters();
                }
                if let HandlerResult::Nack(reason) = result {
                    tracing::warn!("event handler returned Nack: {reason}");
                }
            });
        }
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
