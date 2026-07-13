use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{Notify, RwLock, Semaphore};

use crate::{
    EmitReceipt, EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult,
    RequestOptions, ResponderHandle, SubscriptionHandle, SubscriptionId,
};

use crate::EventFilter;

type Handler = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>, EventMetadata)
        -> Pin<Box<dyn Future<Output = HandlerResult> + Send>>
        + Send
        + Sync,
>;

/// Type-erased request responder: takes the request value + metadata, returns
/// either the reply value (`Box<dyn Any + Send>`) or a remote-error message.
/// The reply is boxed (not `Arc`) so `Resp` need only be `Send`, not `Sync`.
type LocalResponder = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>, EventMetadata)
        -> Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>, String>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
struct HandlerEntry {
    id: u64,
    handler: Handler,
    filter: Option<EventFilter>,
}

/// Default maximum concurrent handlers.
pub const DEFAULT_MAX_CONCURRENCY: usize = 1024;

/// In-process event bus with typed pub/sub and backpressure support.
///
/// Events are dispatched by `TypeId` — subscribers register for a concrete
/// event type and receive an [`EventEnvelope<E>`] when that type is emitted.
///
/// Backpressure is enforced via a semaphore that limits the number of
/// concurrently executing handlers. When the limit is reached, `emit()`
/// will block until a slot becomes available.
///
/// `LocalEventBus` is `Clone` and can be shared across threads.
///
/// **Performance note:** The `Serialize`/`DeserializeOwned` bounds required by
/// the [`EventBus`] trait are compile-time only. `LocalEventBus` never
/// serializes events — dispatch uses `Arc<dyn Any>` downcasting internally.
#[derive(Clone)]
pub struct LocalEventBus {
    handlers: Arc<ArcSwap<HashMap<TypeId, Vec<HandlerEntry>>>>,
    /// At most one responder per request `TypeId` (request-reply).
    responders: Arc<RwLock<HashMap<TypeId, LocalResponder>>>,
    semaphore: Option<Arc<Semaphore>>,
    next_id: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    in_flight_zero: Arc<Notify>,
}

/// Drop-based guard that decrements in_flight and notifies when it reaches zero.
struct InFlightGuard {
    in_flight: Arc<AtomicUsize>,
    in_flight_zero: Arc<Notify>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.in_flight.fetch_sub(1, Ordering::Release) == 1 {
            self.in_flight_zero.notify_waiters();
        }
    }
}

impl LocalEventBus {
    /// Create a new `LocalEventBus` with default concurrency limit (1024).
    pub fn new() -> Self {
        Self::with_concurrency(DEFAULT_MAX_CONCURRENCY)
    }

    /// Create a new `LocalEventBus` with a custom concurrency limit.
    ///
    /// The limit controls how many handlers can execute concurrently across
    /// all event types. When the limit is reached, `emit()` will block until
    /// a handler completes.
    pub fn with_concurrency(max_concurrent: usize) -> Self {
        Self {
            handlers: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            responders: Arc::new(RwLock::new(HashMap::new())),
            semaphore: Some(Arc::new(Semaphore::new(max_concurrent))),
            next_id: Arc::new(AtomicU64::new(1)),
            shutdown: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            in_flight_zero: Arc::new(Notify::new()),
        }
    }

    /// Create a new `LocalEventBus` with no concurrency limit (legacy behavior).
    ///
    /// WARNING: Without backpressure, if events are emitted faster than
    /// handlers can process them, memory usage will grow unbounded.
    pub fn unbounded() -> Self {
        Self {
            handlers: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            responders: Arc::new(RwLock::new(HashMap::new())),
            semaphore: None,
            next_id: Arc::new(AtomicU64::new(1)),
            shutdown: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            in_flight_zero: Arc::new(Notify::new()),
        }
    }

    /// Returns the current concurrency limit, or `None` if unbounded.
    pub fn concurrency_limit(&self) -> Option<usize> {
        self.semaphore
            .as_ref()
            .map(|s| s.available_permits() + self.active_handlers())
    }

    /// Returns the number of currently active (executing) handlers.
    fn active_handlers(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Wait until every in-flight handler has completed.
    ///
    /// `emit` is fire-and-forget: handler tasks are spawned before it returns,
    /// but run asynchronously. This drains them without shutting the bus down —
    /// the primary determinism barrier for tests that emit then assert on
    /// handler side effects. Unlike [`shutdown`](Self::shutdown), the bus stays
    /// usable afterwards.
    pub async fn wait_idle(&self) {
        loop {
            // Register the notified future BEFORE checking the counter to avoid
            // a TOCTOU race where the last handler finishes between the load and
            // the notified() call.
            let notified = self.in_flight_zero.notified();
            if self.in_flight.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Internal: dispatch an event to all matching handlers (fire-and-forget).
    ///
    /// Handler tasks are spawned (permit-bounded) and run asynchronously.
    /// `in_flight` is incremented for each before this returns, so a subsequent
    /// [`wait_idle`](Self::wait_idle) drains exactly the handlers this emit
    /// spawned.
    async fn dispatch(
        &self,
        type_id: TypeId,
        event: Arc<dyn Any + Send + Sync>,
        metadata: EventMetadata,
    ) -> Result<(), EventBusError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(EventBusError::Shutdown);
        }

        // Load a lock-free snapshot of the handler map.
        let handler_snapshot: Vec<Handler> = {
            let map = self.handlers.load();
            match map.get(&type_id) {
                Some(entries) => entries
                    .iter()
                    .filter(|entry| !entry.filter.as_ref().is_some_and(|f| !f(&metadata)))
                    .map(|entry| entry.handler.clone())
                    .collect(),
                None => return Ok(()),
            }
        };

        for h in handler_snapshot {
            let e = event.clone();
            let m = metadata.clone();
            let in_flight = self.in_flight.clone();
            let in_flight_zero = self.in_flight_zero.clone();

            let permit = match &self.semaphore {
                Some(sem) => Some(
                    sem.clone()
                        .acquire_owned()
                        .await
                        .expect("semaphore closed"),
                ),
                None => None,
            };

            // Increment AFTER acquiring the permit and immediately before
            // spawn — if the dispatch future is dropped while awaiting the
            // semaphore, the counter stays accurate and wait_idle won't hang.
            in_flight.fetch_add(1, Ordering::Relaxed);
            r2e_core::rt::spawn(async move {
                let _guard = InFlightGuard { in_flight, in_flight_zero };
                let result = h(e, m).await;
                drop(permit);
                if let HandlerResult::Nack(ref reason) = result {
                    tracing::warn!("event handler returned Nack: {reason}");
                }
            });
        }

        Ok(())
    }
}

impl EventBus for LocalEventBus {
    fn subscribe<E, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<SubscriptionHandle, EventBusError>> + Send
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
        F: Fn(EventEnvelope<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        let handlers = self.handlers.clone();
        let next_id = self.next_id.clone();
        let shutdown = self.shutdown.clone();
        async move {
            if shutdown.load(Ordering::Acquire) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<E>();
            let id = next_id.fetch_add(1, Ordering::Relaxed);

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let handlers_for_unsub = handlers.clone();
            handlers.rcu(|map| {
                let mut new_map = HashMap::clone(map);
                new_map.entry(type_id).or_default().push(HandlerEntry {
                    id,
                    handler: h.clone(),
                    filter: None,
                });
                new_map
            });

            Ok(SubscriptionHandle::new(
                SubscriptionId(id),
                move || {
                    let handlers = handlers_for_unsub.clone();
                    handlers.rcu(move |map| {
                        let mut new_map = HashMap::clone(map);
                        if let Some(entries) = new_map.get_mut(&type_id) {
                            entries.retain(|e| e.id != id);
                        }
                        new_map
                    });
                },
            ))
        }
    }

    fn emit<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let metadata = EventMetadata::new();
        self.dispatch(type_id, event, metadata)
    }

    fn emit_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        self.dispatch(type_id, event, metadata)
    }

    fn emit_nowait<E>(
        &self,
        event: E,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let fut = self.emit(event);
        async move {
            fut.await?;
            Ok(EmitReceipt::ready())
        }
    }

    fn emit_nowait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let fut = self.emit_with(event, metadata);
        async move {
            fut.await?;
            Ok(EmitReceipt::ready())
        }
    }

    fn request_with<Req, Resp>(
        &self,
        req: Req,
        options: RequestOptions,
    ) -> impl Future<Output = Result<Resp, EventBusError>> + Send
    where
        Req: serde::Serialize + Send + Sync + 'static,
        Resp: serde::de::DeserializeOwned + Send + 'static,
    {
        let responders = self.responders.clone();
        let shutdown = self.shutdown.clone();
        let in_flight = self.in_flight.clone();
        let in_flight_zero = self.in_flight_zero.clone();
        async move {
            if shutdown.load(Ordering::Acquire) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<Req>();
            let responder = {
                let map = responders.read().await;
                map.get(&type_id).cloned()
            }
            .ok_or(EventBusError::NoResponder)?;

            let metadata = options.metadata.unwrap_or_default();
            let req_any = Arc::new(req) as Arc<dyn Any + Send + Sync>;

            // Invoke on the control plane with in-flight tracking, mirroring the
            // emit dispatch discipline; time out waiting for the reply.
            in_flight.fetch_add(1, Ordering::Relaxed);
            let handle = r2e_core::rt::spawn(async move {
                let _guard = InFlightGuard { in_flight, in_flight_zero };
                responder(req_any, metadata).await
            });

            match r2e_core::rt::timeout(options.timeout, handle).await {
                Err(_) => Err(EventBusError::RequestTimeout),
                Ok(Err(_join)) => {
                    Err(EventBusError::Other("responder task panicked".to_string()))
                }
                Ok(Ok(Err(msg))) => Err(EventBusError::Remote(msg)),
                Ok(Ok(Ok(reply))) => match reply.downcast::<Resp>() {
                    Ok(boxed) => Ok(*boxed),
                    Err(_) => Err(EventBusError::Serialization(
                        "reply type does not match the requested response type".to_string(),
                    )),
                },
            }
        }
    }

    fn respond<Req, Resp, E, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<ResponderHandle, EventBusError>> + Send
    where
        Req: serde::de::DeserializeOwned + Send + Sync + 'static,
        Resp: serde::Serialize + Send + 'static,
        E: std::fmt::Display + Send + 'static,
        F: Fn(EventEnvelope<Req>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, E>> + Send + 'static,
    {
        let responders = self.responders.clone();
        let shutdown = self.shutdown.clone();
        async move {
            if shutdown.load(Ordering::Acquire) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<Req>();
            let type_name = std::any::type_name::<Req>();

            let responder: LocalResponder = Arc::new(move |any, metadata| {
                let event = any.downcast::<Req>().expect("request type mismatch");
                let envelope = EventEnvelope { event, metadata };
                let fut = handler(envelope);
                Box::pin(async move {
                    match fut.await {
                        Ok(resp) => Ok(Box::new(resp) as Box<dyn Any + Send>),
                        Err(e) => Err(e.to_string()),
                    }
                })
            });

            let mut map = responders.write().await;
            if map.contains_key(&type_id) {
                return Err(EventBusError::Other(format!(
                    "a responder is already registered for request type `{type_name}`"
                )));
            }
            map.insert(type_id, responder);
            drop(map);

            let responders_for_unreg = responders.clone();
            Ok(ResponderHandle::new(type_name, move || {
                let responders = responders_for_unreg.clone();
                // Unregister may be triggered from a request handler, so route
                // to the control plane in sharded mode.
                r2e_core::rt::spawn_ctl(async move {
                    responders.write().await.remove(&type_id);
                });
            }))
        }
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let handlers = self.handlers.clone();
        let responders = self.responders.clone();
        async move {
            handlers.store(Arc::new(HashMap::new()));
            responders.write().await.clear();
        }
    }

    fn shutdown(
        &self,
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let shutdown = self.shutdown.clone();
        let in_flight = self.in_flight.clone();
        let in_flight_zero = self.in_flight_zero.clone();
        let handlers = self.handlers.clone();
        async move {
            shutdown.store(true, Ordering::Release);

            if in_flight.load(Ordering::Acquire) > 0 {
                let wait = async {
                    loop {
                        let notified = in_flight_zero.notified();
                        if in_flight.load(Ordering::Acquire) == 0 {
                            return;
                        }
                        notified.await;
                    }
                };
                if r2e_core::rt::timeout(timeout, wait).await.is_err() {
                    handlers.store(Arc::new(HashMap::new()));
                    return Err(EventBusError::Other(format!(
                        "shutdown timed out with {} handlers still in flight",
                        in_flight.load(Ordering::Acquire)
                    )));
                }
            }

            handlers.store(Arc::new(HashMap::new()));
            Ok(())
        }
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new()
    }
}
