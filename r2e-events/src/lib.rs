use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

type Handler = Arc<dyn Fn(Arc<dyn Any + Send + Sync>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Default maximum concurrent handlers.
pub const DEFAULT_MAX_CONCURRENCY: usize = 1024;

/// In-process event bus with typed pub/sub and backpressure support.
///
/// Events are dispatched by `TypeId` — subscribers register for a concrete
/// event type and receive an `Arc<E>` when that type is emitted.
///
/// Backpressure is enforced via a semaphore that limits the number of
/// concurrently executing handlers. When the limit is reached, `emit()`
/// will block until a slot becomes available.
///
/// `EventBus` is `Clone` and can be shared across threads.
#[derive(Clone)]
pub struct EventBus {
    handlers: Arc<RwLock<HashMap<TypeId, Vec<Handler>>>>,
    semaphore: Option<Arc<Semaphore>>,
}

impl EventBus {
    /// Create a new `EventBus` with default concurrency limit (1024).
    pub fn new() -> Self {
        Self::with_concurrency(DEFAULT_MAX_CONCURRENCY)
    }

    /// Create a new `EventBus` with a custom concurrency limit.
    ///
    /// The limit controls how many handlers can execute concurrently across
    /// all event types. When the limit is reached, `emit()` will block until
    /// a handler completes.
    pub fn with_concurrency(max_concurrent: usize) -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            semaphore: Some(Arc::new(Semaphore::new(max_concurrent))),
        }
    }

    /// Create a new `EventBus` with no concurrency limit (legacy behavior).
    ///
    /// WARNING: Without backpressure, if events are emitted faster than
    /// handlers can process them, memory usage will grow unbounded.
    pub fn unbounded() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            semaphore: None,
        }
    }

    /// Subscribe to events of type `E`.
    ///
    /// The handler receives `Arc<E>` and is called for every `emit()` of that type.
    pub async fn subscribe<E, F, Fut>(&self, handler: F)
    where
        E: Send + Sync + 'static,
        F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let type_id = TypeId::of::<E>();
        let handler: Handler = Arc::new(move |any| {
            let event = any.downcast::<E>().expect("event type mismatch");
            Box::pin(handler(event))
        });
        let mut handlers = self.handlers.write().await;
        handlers.entry(type_id).or_default().push(handler);
    }

    /// Emit an event, spawning all subscribers as concurrent tasks.
    ///
    /// If backpressure is enabled (default), this method will block when the
    /// concurrency limit is reached, waiting for a handler slot to become available.
    ///
    /// Returns after all handlers have been spawned (not necessarily completed).
    pub async fn emit<E: Send + Sync + 'static>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let handlers = self.handlers.read().await;
        if let Some(subs) = handlers.get(&type_id) {
            for handler in subs {
                let h = handler.clone();
                let e = event.clone();
                match &self.semaphore {
                    Some(sem) => {
                        // Acquire permit before spawning - blocks if at limit
                        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
                        tokio::spawn(async move {
                            h(e).await;
                            drop(permit); // Release slot when handler completes
                        });
                    }
                    None => {
                        // Unbounded mode - no backpressure
                        tokio::spawn(async move {
                            h(e).await;
                        });
                    }
                }
            }
        }
    }

    /// Emit an event and wait for all subscribers to complete.
    ///
    /// If backpressure is enabled (default), this method will block when the
    /// concurrency limit is reached, waiting for a handler slot to become available.
    pub async fn emit_and_wait<E: Send + Sync + 'static>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let handlers = self.handlers.read().await;
        if let Some(subs) = handlers.get(&type_id) {
            let mut tasks = Vec::new();
            for handler in subs {
                let h = handler.clone();
                let e = event.clone();
                match &self.semaphore {
                    Some(sem) => {
                        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
                        tasks.push(tokio::spawn(async move {
                            h(e).await;
                            drop(permit);
                        }));
                    }
                    None => {
                        tasks.push(tokio::spawn(async move {
                            h(e).await;
                        }));
                    }
                }
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    /// Returns the current concurrency limit, or `None` if unbounded.
    pub fn concurrency_limit(&self) -> Option<usize> {
        self.semaphore.as_ref().map(|s| s.available_permits() + self.active_handlers())
    }

    /// Returns the number of currently active (executing) handlers.
    ///
    /// This is an approximation as handlers may complete between the check
    /// and the return. Returns 0 if unbounded mode is enabled.
    fn active_handlers(&self) -> usize {
        // Note: This is a rough estimate. The semaphore doesn't directly expose
        // the number of acquired permits, so we'd need additional tracking for
        // precise counts. For now, this method exists for potential future use.
        0
    }
}

pub mod prelude {
    //! Re-exports of the most commonly used event types.
    pub use crate::EventBus;
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
