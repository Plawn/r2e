use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::{Notify, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use super::dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
use super::topic::TopicRegistry;
use crate::{
    DlqPublisher, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
    SubscriptionId,
};

/// Type-erased request responder for distributed backends.
///
/// Given the raw request payload bytes and its decoded metadata, it
/// deserializes the request, invokes the user handler, and serializes the
/// reply — yielding the reply bytes or a remote-error message. A backend's
/// request-topic consumer only needs bytes-in / reply-bytes-out plus a
/// "publish reply to `<reply-to>`" callback of its own.
pub type ResponderFn = Arc<
    dyn Fn(&[u8], EventMetadata) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send>>
        + Send
        + Sync,
>;

/// Default maximum concurrent handlers for distributed backends.
pub const DEFAULT_BACKEND_CONCURRENCY: usize = 1024;

/// Per-message outcome of dispatching to local handlers.
///
/// Drives the broker ack/commit decision for at-least-once delivery:
/// pollers ack/commit on [`DispatchOutcome::Ack`] and skip the ack (or
/// negative-ack) on [`DispatchOutcome::Nack`] so the broker redelivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Safe to ack/commit: every handler acked (after retries), every nacked
    /// handler's payload was captured to a DLQ, there were no matching
    /// handlers, or the payload failed to deserialize and was either durably
    /// parked in every configured DLQ or intentionally dropped because no DLQ
    /// was configured.
    Ack,
    /// At least one handler nacked (or panicked) without DLQ capture —
    /// skip the ack/commit so the broker redelivers the message.
    Nack,
}

/// Completion signal for one dispatched message.
///
/// Returned by [`BackendState::dispatch_from_poller_tracked`] immediately
/// after the handler tasks are spawned (permit-bounded), so the poller can
/// keep pulling messages while handlers run. Await [`outcome`] — typically
/// from a spawned follow-up task or an ordered ack pipeline — to learn
/// whether the message can be acked/committed.
///
/// [`outcome`]: DispatchCompletion::outcome
pub struct DispatchCompletion {
    resolved: Option<DispatchOutcome>,
    receivers: Vec<tokio::sync::oneshot::Receiver<bool>>,
}

impl DispatchCompletion {
    /// A completion that resolves immediately with the given outcome.
    pub fn resolved(outcome: DispatchOutcome) -> Self {
        Self {
            resolved: Some(outcome),
            receivers: Vec::new(),
        }
    }

    /// Resolve once every handler spawned for this message has finished.
    ///
    /// A handler task that panics counts as a nack (the message will be
    /// redelivered).
    pub async fn outcome(self) -> DispatchOutcome {
        if let Some(outcome) = self.resolved {
            return outcome;
        }
        let mut outcome = DispatchOutcome::Ack;
        for rx in self.receivers {
            match rx.await {
                Ok(true) => {}
                // false = nack without DLQ capture; Err = handler task panicked.
                Ok(false) | Err(_) => outcome = DispatchOutcome::Nack,
            }
        }
        outcome
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
    /// Per-TypeId handler registry (tokio RwLock — may be held across async dispatch).
    pub handlers: RwLock<HashMap<TypeId, TopicHandlers>>,
    /// TypeId -> resolved topic name (std RwLock — never held across awaits).
    pub topic_registry: std::sync::RwLock<TopicRegistry>,
    /// Cancellation tokens for background pollers, keyed by TypeId.
    pub poller_cancels: Mutex<HashMap<TypeId, CancellationToken>>,
    /// Number of handlers currently executing.
    pub in_flight: AtomicUsize,
    /// Notified when `in_flight` drops to zero.
    pub in_flight_zero: Notify,
    /// Set of topics already ensured to exist in the backend. Uses
    /// `std::sync::RwLock` so the hot-path `is_topic_ensured` check takes a
    /// shared read lock (no contention with concurrent emits).
    pub ensured_topics: std::sync::RwLock<HashSet<String>>,
    /// Request-reply responders, keyed by request `TypeId`. At most one per
    /// type (enforced by [`register_responder`](BackendState::register_responder)).
    pub responders: RwLock<HashMap<TypeId, ResponderFn>>,
    /// Optional callback for publishing failed events to a dead-letter topic.
    /// Provided by the backend when constructing `BackendState`.
    pub dlq_publisher: Option<DlqPublisher>,
    /// Semaphore limiting concurrent handler execution (backpressure).
    pub handler_semaphore: Arc<Semaphore>,
}

/// Default capacity for a poller's completion channel — bounds how many
/// resolved-but-unapplied ack decisions may queue before completion
/// forwarders apply backpressure.
pub const COMPLETION_CHANNEL_CAPACITY: usize = 1024;

/// Best-effort deadline for draining pending completion decisions when a
/// poller shuts down. Undrained messages are simply redelivered.
pub const COMPLETION_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Spawn a task that awaits a [`DispatchCompletion`] and forwards
/// `(key, outcome)` to the poller's completion channel.
///
/// This is the shared consume-loop pattern: the poller stays pipelined (keeps
/// pulling messages) while completions resolve out of order; the loop applies
/// each decision (offset store / broker ack) from its `select!` arm. `key`
/// identifies the message in backend terms (e.g. `(partition, offset)` for
/// Kafka, `(topic, message_id)` for Pulsar). Send errors are ignored: the
/// receiver only closes when the poller has exited, and unapplied decisions
/// just mean redelivery.
pub fn spawn_completion_forwarder<K: Send + 'static>(
    completion: DispatchCompletion,
    key: K,
    tx: tokio::sync::mpsc::Sender<(K, DispatchOutcome)>,
) {
    r2e_core::rt::spawn(async move {
        let outcome = completion.outcome().await;
        let _ = tx.send((key, outcome)).await;
    });
}

/// RAII guard that decrements `in_flight` and notifies waiters on drop.
///
/// Created by [`BackendState::acquire_in_flight`]. Ensures the in-flight
/// counter is always decremented even if the handler panics.
pub struct InFlightGuard {
    state: Arc<BackendState>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.state.in_flight.fetch_sub(1, Ordering::Release) == 1 {
            self.state.in_flight_zero.notify_waiters();
        }
    }
}

impl BackendState {
    /// Create a new `BackendState` with the given topic registry.
    pub fn new(topic_registry: TopicRegistry) -> Self {
        Self::with_dlq_publisher(topic_registry, None)
    }

    /// Create a new `BackendState` with a DLQ publisher callback.
    pub fn with_dlq_publisher(
        topic_registry: TopicRegistry,
        dlq_publisher: Option<DlqPublisher>,
    ) -> Self {
        Self::with_options(topic_registry, dlq_publisher, DEFAULT_BACKEND_CONCURRENCY)
    }

    /// Create a new `BackendState` with a DLQ publisher and custom concurrency limit.
    pub fn with_options(
        topic_registry: TopicRegistry,
        dlq_publisher: Option<DlqPublisher>,
        max_concurrency: usize,
    ) -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            handlers: RwLock::new(HashMap::new()),
            topic_registry: std::sync::RwLock::new(topic_registry),
            poller_cancels: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            in_flight_zero: Notify::new(),
            ensured_topics: std::sync::RwLock::new(HashSet::new()),
            responders: RwLock::new(HashMap::new()),
            dlq_publisher,
            handler_semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    /// Check if the bus is shut down, returning `Err(Shutdown)` if so.
    pub fn check_shutdown(&self) -> Result<(), EventBusError> {
        if self.shutdown.load(Ordering::Acquire) {
            Err(EventBusError::Shutdown)
        } else {
            Ok(())
        }
    }

    /// Allocate the next handler ID.
    pub fn next_handler_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Increment `in_flight` and return an RAII guard that decrements it on drop.
    ///
    /// The guard ensures panic-safety: even if the handler panics, the counter
    /// is decremented and waiters are notified.
    pub fn acquire_in_flight(self: &Arc<Self>) -> InFlightGuard {
        self.in_flight.fetch_add(1, Ordering::Release);
        InFlightGuard {
            state: self.clone(),
        }
    }

    /// Register a cancellation token for a background poller.
    ///
    /// Returns the token — pass it to the poller task. Call `.cancel()` to stop
    /// the poller (done automatically by `cancel_all_pollers` on shutdown).
    pub fn register_poller_cancel(&self, type_id: TypeId) -> CancellationToken {
        let cancel = CancellationToken::new();
        self.poller_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(type_id, cancel.clone());
        cancel
    }

    /// Resolve the topic name for an event type.
    ///
    /// Uses `std::sync::RwLock` (not async) — this is a fast HashMap lookup
    /// called on every emit, so we avoid Tokio runtime overhead. Returns
    /// `Arc<str>` so callers can hold the name cheaply without cloning.
    pub fn resolve_topic<E: 'static>(&self) -> Arc<str> {
        let type_id = TypeId::of::<E>();
        let type_name = std::any::type_name::<E>();
        {
            let reg = self
                .topic_registry
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(topic) = reg.get(type_id) {
                return topic;
            }
        }

        self.topic_registry
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(type_id, type_name)
    }

    /// Check if a topic has already been ensured.
    ///
    /// Returns `true` if the topic was previously marked as ensured.
    pub fn is_topic_ensured(&self, topic_name: &str) -> bool {
        self.ensured_topics
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(topic_name)
    }

    /// Mark a topic as ensured (call after successful creation).
    pub fn set_topic_ensured(&self, topic_name: &str) {
        self.ensured_topics
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(topic_name.to_string());
    }

    /// Register a handler for an event type, returning `(handler_id, is_first_for_type)`.
    pub async fn register_handler<E>(&self, handler: Handler) -> (u64, bool)
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        let json_deser: DeserializerFn = Arc::new(|bytes: &[u8]| {
            serde_json::from_slice::<E>(bytes)
                .map(|e| Arc::new(e) as Arc<dyn std::any::Any + Send + Sync>)
                .map_err(|e| e.to_string())
        });
        self.register_handler_inner::<E>(handler, json_deser, None, None)
            .await
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
        self.register_handler_inner::<E>(handler, deserializer, None, None)
            .await
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
        self.register_handler_inner::<E>(handler, deser, filter, retry_policy)
            .await
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

        let topic_entry = map.entry(type_id).or_insert_with(|| TopicHandlers {
            entries: Vec::new(),
            deserializer,
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
            // Unsubscribe can be triggered from a request handler, so route to
            // the control plane in sharded mode.
            r2e_core::rt::spawn_ctl(async move {
                state.unregister_handler(type_id, handler_id).await;
            });
        })
    }

    /// Remove a handler immediately, cancelling its poller when it was the last
    /// handler for the type. Used both by subscription handles and to roll back
    /// a registration when first-subscriber broker setup fails.
    pub async fn unregister_handler(&self, type_id: TypeId, handler_id: u64) {
        let mut map = self.handlers.write().await;
        let remove_type = if let Some(th) = map.get_mut(&type_id) {
            th.entries.retain(|e| e.id != handler_id);
            th.entries.is_empty()
        } else {
            false
        };
        if remove_type {
            map.remove(&type_id);
            let mut cancels = self
                .poller_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(cancel) = cancels.remove(&type_id) {
                cancel.cancel();
            }
        }
    }

    /// Register the single request-reply responder for request type `Req`.
    ///
    /// Returns an error if a responder is already registered for `Req` (at most
    /// one responder per request type per process). The stored [`ResponderFn`]
    /// deserializes the request bytes, invokes `handler`, and serializes the
    /// reply — see [`invoke_responder`](Self::invoke_responder).
    pub async fn register_responder<Req, Resp, E, F, Fut>(
        &self,
        handler: F,
    ) -> Result<(), EventBusError>
    where
        Req: DeserializeOwned + Send + Sync + 'static,
        Resp: Serialize + Send + 'static,
        E: std::fmt::Display + Send + 'static,
        F: Fn(EventEnvelope<Req>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, E>> + Send + 'static,
    {
        let type_id = TypeId::of::<Req>();
        let type_name = std::any::type_name::<Req>();

        let responder: ResponderFn = Arc::new(
            move |bytes, metadata| match serde_json::from_slice::<Req>(bytes) {
                Ok(req) => {
                    let envelope = EventEnvelope {
                        event: Arc::new(req),
                        metadata: Arc::new(metadata),
                    };
                    let fut = handler(envelope);
                    Box::pin(async move {
                        match fut.await {
                            Ok(resp) => serde_json::to_vec(&resp).map_err(|e| e.to_string()),
                            Err(e) => Err(e.to_string()),
                        }
                    })
                }
                Err(e) => {
                    let msg = format!("failed to deserialize request: {e}");
                    Box::pin(async move { Err(msg) })
                }
            },
        );

        let mut map = self.responders.write().await;
        if map.contains_key(&type_id) {
            return Err(EventBusError::Other(format!(
                "a responder is already registered for request type `{type_name}`"
            )));
        }
        map.insert(type_id, responder);
        Ok(())
    }

    /// Remove the responder registered for request type identified by `type_id`.
    pub async fn unregister_responder(&self, type_id: TypeId) {
        self.responders.write().await.remove(&type_id);
    }

    /// Invoke the responder for `type_id` with the raw request `payload` and
    /// its `metadata`, returning the reply bytes or a remote-error message.
    ///
    /// Returns `None` when no responder is registered for `type_id` — the
    /// backend's request consumer should surface this as "no responder".
    pub async fn invoke_responder(
        &self,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) -> Option<Result<Vec<u8>, String>> {
        let responder = {
            let map = self.responders.read().await;
            map.get(&type_id).cloned()
        };
        match responder {
            Some(r) => Some(r(payload, metadata).await),
            None => None,
        }
    }

    /// Build the reply payload and optional error for a request of `type_id`,
    /// single-sourcing the responder-outcome mapping every backend otherwise
    /// hand-rolls.
    ///
    /// Wraps [`invoke_responder`](Self::invoke_responder):
    /// - responder succeeded → `(reply_bytes, None)`;
    /// - responder returned an error → `(b"null", Some(msg))`;
    /// - **no responder registered** → `(b"null", Some("no responder registered
    ///   for request type"))` — a missing responder ALWAYS produces an error
    ///   reply, never a silent drop, so the requester surfaces
    ///   [`EventBusError::Remote`] instead of waiting out the timeout.
    ///
    /// The `b"null"` error placeholder is a valid non-empty JSON payload (some
    /// brokers reject empty payloads); it is ignored by the requester, which
    /// reads the outcome from the reply-error header. Callers publish the reply
    /// with `encode_reply_headers(request_id, None, error.as_deref())`.
    pub async fn build_reply(
        &self,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) -> (Vec<u8>, Option<String>) {
        match self.invoke_responder(type_id, payload, metadata).await {
            Some(Ok(bytes)) => (bytes, None),
            Some(Err(msg)) => (b"null".to_vec(), Some(msg)),
            None => (
                b"null".to_vec(),
                Some("no responder registered for request type".to_string()),
            ),
        }
    }

    /// Dispatch a message from a poller to local handlers (fire-and-forget with in-flight tracking).
    ///
    /// Identical to [`dispatch_from_poller_tracked`] except the per-message
    /// outcome is discarded — use the tracked variant when the poller needs
    /// to ack/commit based on handler results (at-least-once delivery).
    ///
    /// [`dispatch_from_poller_tracked`]: Self::dispatch_from_poller_tracked
    pub async fn dispatch_from_poller(
        self: &Arc<Self>,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) {
        // Dropping the completion is fine: handler tasks are already spawned.
        let _ = self
            .dispatch_from_poller_tracked(type_id, payload, metadata)
            .await;
    }

    /// Dispatch a message from a poller to local handlers, returning a
    /// per-message [`DispatchCompletion`].
    ///
    /// Returns as soon as all handler tasks are spawned (permit-bounded), so
    /// the poll loop stays pipelined. The completion resolves when every
    /// handler for the message has finished: [`DispatchOutcome::Ack`] when the
    /// broker copy can be acked/committed, [`DispatchOutcome::Nack`] when at
    /// least one handler failed without DLQ capture and the broker should
    /// redeliver.
    ///
    /// Backpressure: a semaphore permit is acquired **before** spawning each handler
    /// task, so the poller naturally slows down when handlers are saturated.
    ///
    /// Panic-safety: each task holds an [`InFlightGuard`] that decrements the
    /// in-flight counter on drop, even on panic; a panicked handler resolves
    /// the completion as [`DispatchOutcome::Nack`].
    pub async fn dispatch_from_poller_tracked(
        self: &Arc<Self>,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) -> DispatchCompletion {
        // Clone handler data under the read lock, then release before
        // deserializing so the CPU-bound serde work doesn't block
        // subscribe/unsubscribe.
        let (deserializer, handler_data, dlq_data) = {
            let map = self.handlers.read().await;
            let topic_handlers = match map.get(&type_id) {
                Some(th) => th,
                None => return DispatchCompletion::resolved(DispatchOutcome::Ack),
            };

            let deser = topic_handlers.deserializer.clone();

            let handlers: Vec<_> = topic_handlers
                .entries
                .iter()
                .filter(|entry| !entry.filter.as_ref().is_some_and(|f| !f(&metadata)))
                .map(|entry| {
                    (
                        entry.handler.clone(),
                        entry.retry_policy.clone(),
                        entry
                            .retry_policy
                            .as_ref()
                            .and_then(|p| p.dead_letter_topic.clone()),
                    )
                })
                .collect();

            // Pre-allocate DLQ data only if any handler has a DLQ configured.
            let has_dlq =
                self.dlq_publisher.is_some() && handlers.iter().any(|(_, _, dlq)| dlq.is_some());
            let dlq_data: Option<(Arc<Vec<u8>>, EventMetadata)> = if has_dlq {
                Some((Arc::new(payload.to_vec()), metadata.clone()))
            } else {
                None
            };

            (deser, handlers, dlq_data)
        };
        // RwLock released — deserialize outside the lock.

        let event = match deserializer(payload) {
            Ok(e) => e,
            Err(err) => {
                tracing::error!("failed to deserialize event: {err}");
                let mut dlq_topics: Vec<String> = handler_data
                    .iter()
                    .filter_map(|(_, _, dlq)| dlq.clone())
                    .collect();
                dlq_topics.sort();
                dlq_topics.dedup();
                let mut parked = !dlq_topics.is_empty();
                if let Some(ref publisher) = self.dlq_publisher {
                    for topic in dlq_topics {
                        if let Err(error) =
                            publisher(topic.clone(), payload.to_vec(), metadata.clone()).await
                        {
                            tracing::error!(topic = %topic, %error, "failed to park poison message in DLQ");
                            parked = false;
                        }
                    }
                } else if parked {
                    tracing::error!(
                        "poison message has a configured DLQ but no DLQ publisher is available"
                    );
                    parked = false;
                }
                // Without a configured DLQ, dropping an undecodable poison
                // message is intentional. With a DLQ, acknowledge only after
                // every broker publication succeeded.
                return DispatchCompletion::resolved(
                    if parked || handler_data.iter().all(|(_, _, dlq)| dlq.is_none()) {
                        DispatchOutcome::Ack
                    } else {
                        DispatchOutcome::Nack
                    },
                );
            }
        };

        let mut receivers = Vec::with_capacity(handler_data.len());
        for (h, retry_policy, _dlq_topic) in handler_data {
            let e = event.clone();
            let m = metadata.clone();
            let state = self.clone();
            let dlq_data = dlq_data.clone();

            // Backpressure: acquire permit BEFORE spawning to bound task count.
            let permit = self
                .handler_semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore closed");

            let guard = self.acquire_in_flight();

            let (tx, rx) = tokio::sync::oneshot::channel();
            receivers.push(rx);

            // The poller loop already runs on the control plane (registered at
            // startup), so plain `spawn` keeps handler tasks there.
            r2e_core::rt::spawn(async move {
                let _guard = guard;
                let result = if let Some(ref policy) = retry_policy {
                    Self::invoke_with_retry(&h, &e, &m, policy).await
                } else {
                    h(e, m).await
                };
                let acked = match result {
                    HandlerResult::Ack => true,
                    HandlerResult::Nack(ref reason) => {
                        tracing::warn!("event handler returned Nack: {reason}");
                        let mut captured = false;
                        if let Some(ref policy) = retry_policy {
                            if let Some(ref dlq_topic) = policy.dead_letter_topic {
                                if let (Some((ref pl, ref meta)), Some(ref publisher)) =
                                    (&dlq_data, &state.dlq_publisher)
                                {
                                    match publisher(
                                        dlq_topic.clone(),
                                        pl.as_ref().clone(),
                                        meta.clone(),
                                    )
                                    .await
                                    {
                                        Ok(()) => captured = true,
                                        Err(error) => tracing::error!(
                                            topic = %dlq_topic,
                                            %error,
                                            "failed to publish exhausted event to DLQ"
                                        ),
                                    }
                                }
                            }
                        }
                        captured
                    }
                };
                drop(permit);
                // Receiver may be gone (untracked dispatch) — ignore send errors.
                let _ = tx.send(acked);
            });
        }
        DispatchCompletion {
            resolved: None,
            receivers,
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
                if let Some(entry) = topic_handlers
                    .entries
                    .iter_mut()
                    .find(|e| e.id == handler_id)
                {
                    entry.filter = filter;
                    entry.retry_policy = retry_policy;
                    return;
                }
            }
        }
        // Fallback: scan all types
        for topic_handlers in map.values_mut() {
            if let Some(entry) = topic_handlers
                .entries
                .iter_mut()
                .find(|e| e.id == handler_id)
            {
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
            r2e_core::rt::sleep(delay).await;

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
    pub fn cancel_all_pollers(&self) {
        let cancels = std::mem::take(
            &mut *self
                .poller_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for cancel in cancels.into_values() {
            cancel.cancel();
        }
    }

    /// Wait for all in-flight handlers to complete, with timeout.
    ///
    /// Returns `Ok(())` if all handlers finished, or `Err` if timed out.
    pub async fn wait_in_flight(&self, timeout: std::time::Duration) -> Result<(), EventBusError> {
        let wait = async {
            loop {
                // Register the notified future BEFORE checking the counter
                // to avoid a TOCTOU race where the last handler finishes
                // between our load and the notified() call.
                let notified = self.in_flight_zero.notified();
                if self.in_flight.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        if r2e_core::rt::timeout(timeout, wait).await.is_err() {
            self.handlers.write().await.clear();
            return Err(EventBusError::Other(format!(
                "shutdown timed out with {} handlers still in flight",
                self.in_flight.load(Ordering::Acquire)
            )));
        }
        Ok(())
    }
}
