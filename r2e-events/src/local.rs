use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Notify, RwLock, Semaphore};

use crate::{EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle, SubscriptionId};

use crate::EventFilter;

type Handler = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>, EventMetadata)
        -> Pin<Box<dyn Future<Output = HandlerResult> + Send>>
        + Send
        + Sync,
>;

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
    handlers: Arc<RwLock<HashMap<TypeId, Vec<HandlerEntry>>>>,
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
        if self.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
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
            handlers: Arc::new(RwLock::new(HashMap::new())),
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
            handlers: Arc::new(RwLock::new(HashMap::new())),
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
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Internal: dispatch event to all handlers, optionally waiting for completion.
    async fn dispatch(
        &self,
        type_id: TypeId,
        event: Arc<dyn Any + Send + Sync>,
        metadata: EventMetadata,
        wait: bool,
    ) -> Result<(), EventBusError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(EventBusError::Shutdown);
        }

        let map = self.handlers.read().await;
        if let Some(entries) = map.get(&type_id) {
            let mut tasks = Vec::new();
            for entry in entries {
                // Check filter
                if entry.filter.as_ref().is_some_and(|f| !f(&metadata)) {
                    continue;
                }
                let h = entry.handler.clone();
                let e = event.clone();
                let m = metadata.clone();
                let in_flight = self.in_flight.clone();
                let in_flight_zero = self.in_flight_zero.clone();

                in_flight.fetch_add(1, Ordering::SeqCst);

                match &self.semaphore {
                    Some(sem) => {
                        let permit = sem
                            .clone()
                            .acquire_owned()
                            .await
                            .expect("semaphore closed");
                        let handle = tokio::spawn(async move {
                            let _guard = InFlightGuard { in_flight, in_flight_zero };
                            let result = h(e, m).await;
                            drop(permit);
                            result
                        });
                        tasks.push(handle);
                    }
                    None => {
                        let handle = tokio::spawn(async move {
                            let _guard = InFlightGuard { in_flight, in_flight_zero };
                            h(e, m).await
                        });
                        tasks.push(handle);
                    }
                }
            }

            if wait {
                for task in tasks {
                    if let Ok(HandlerResult::Nack(reason)) = task.await {
                        tracing::warn!("event handler returned Nack: {reason}");
                    }
                }
            } else {
                // Fire-and-forget, but still log Nacks
                for task in tasks {
                    tokio::spawn(async move {
                        if let Ok(HandlerResult::Nack(reason)) = task.await {
                            tracing::warn!("event handler returned Nack: {reason}");
                        }
                    });
                }
            }
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
            if shutdown.load(Ordering::SeqCst) {
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
            let mut map = handlers.write().await;
            map.entry(type_id).or_default().push(HandlerEntry {
                id,
                handler: h,
                filter: None,
            });

            Ok(SubscriptionHandle::new(
                SubscriptionId(id),
                move || {
                    let handlers = handlers_for_unsub.clone();
                    // Always spawn a task to avoid borrow issues with try_write.
                    tokio::spawn(async move {
                        let mut map = handlers.write().await;
                        if let Some(entries) = map.get_mut(&type_id) {
                            entries.retain(|e| e.id != id);
                        }
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
        self.dispatch(type_id, event, metadata, false)
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
        self.dispatch(type_id, event, metadata, false)
    }

    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let metadata = EventMetadata::new();
        self.dispatch(type_id, event, metadata, true)
    }

    fn emit_and_wait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        self.dispatch(type_id, event, metadata, true)
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let handlers = self.handlers.clone();
        async move {
            let mut map = handlers.write().await;
            map.clear();
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
            // Set the shutdown flag
            shutdown.store(true, Ordering::SeqCst);

            // Wait for in-flight handlers to complete (with timeout)
            if in_flight.load(Ordering::SeqCst) > 0 {
                let wait = async {
                    loop {
                        if in_flight.load(Ordering::SeqCst) == 0 {
                            return;
                        }
                        in_flight_zero.notified().await;
                    }
                };
                if tokio::time::timeout(timeout, wait).await.is_err() {
                    // Clear handlers anyway
                    handlers.write().await.clear();
                    return Err(EventBusError::Other(format!(
                        "shutdown timed out with {} handlers still in flight",
                        in_flight.load(Ordering::SeqCst)
                    )));
                }
            }

            // Clear all handlers
            handlers.write().await.clear();
            Ok(())
        }
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new()
    }
}
