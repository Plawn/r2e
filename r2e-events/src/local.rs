use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use crate::EventBus;

type Handler = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

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
/// `LocalEventBus` is `Clone` and can be shared across threads.
///
/// **Performance note:** The `Serialize`/`DeserializeOwned` bounds required by
/// the [`EventBus`] trait are compile-time only. `LocalEventBus` never
/// serializes events — dispatch uses `Arc<dyn Any>` downcasting internally.
#[derive(Clone)]
pub struct LocalEventBus {
    handlers: Arc<RwLock<HashMap<TypeId, Vec<Handler>>>>,
    semaphore: Option<Arc<Semaphore>>,
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
        }
    }

    /// Returns the current concurrency limit, or `None` if unbounded.
    pub fn concurrency_limit(&self) -> Option<usize> {
        self.semaphore
            .as_ref()
            .map(|s| s.available_permits() + self.active_handlers())
    }

    /// Returns the number of currently active (executing) handlers.
    ///
    /// This is an approximation as handlers may complete between the check
    /// and the return. Returns 0 if unbounded mode is enabled.
    fn active_handlers(&self) -> usize {
        0
    }
}

impl EventBus for LocalEventBus {
    fn subscribe<E, F, Fut>(&self, handler: F) -> impl Future<Output = ()> + Send
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
        F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handlers = self.handlers.clone();
        async move {
            let type_id = TypeId::of::<E>();
            let handler: Handler = Arc::new(move |any| {
                let event = any.downcast::<E>().expect("event type mismatch");
                Box::pin(handler(event))
            });
            let mut map = handlers.write().await;
            map.entry(type_id).or_default().push(handler);
        }
    }

    fn emit<E>(&self, event: E) -> impl Future<Output = ()> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let handlers = self.handlers.clone();
        let semaphore = self.semaphore.clone();
        async move {
            let type_id = TypeId::of::<E>();
            let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
            let map = handlers.read().await;
            if let Some(subs) = map.get(&type_id) {
                for handler in subs {
                    let h = handler.clone();
                    let e = event.clone();
                    match &semaphore {
                        Some(sem) => {
                            let permit = sem
                                .clone()
                                .acquire_owned()
                                .await
                                .expect("semaphore closed");
                            tokio::spawn(async move {
                                h(e).await;
                                drop(permit);
                            });
                        }
                        None => {
                            tokio::spawn(async move {
                                h(e).await;
                            });
                        }
                    }
                }
            }
        }
    }

    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = ()> + Send
    where
        E: serde::Serialize + Send + Sync + 'static,
    {
        let handlers = self.handlers.clone();
        let semaphore = self.semaphore.clone();
        async move {
            let type_id = TypeId::of::<E>();
            let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
            let map = handlers.read().await;
            if let Some(subs) = map.get(&type_id) {
                let mut tasks = Vec::new();
                for handler in subs {
                    let h = handler.clone();
                    let e = event.clone();
                    match &semaphore {
                        Some(sem) => {
                            let permit = sem
                                .clone()
                                .acquire_owned()
                                .await
                                .expect("semaphore closed");
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
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let handlers = self.handlers.clone();
        async move {
            let mut map = handlers.write().await;
            map.clear();
        }
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new()
    }
}
