use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

type Handler = Arc<dyn Fn(Arc<dyn Any + Send + Sync>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// In-process event bus with typed pub/sub.
///
/// Events are dispatched by `TypeId` — subscribers register for a concrete
/// event type and receive an `Arc<E>` when that type is emitted.
///
/// `EventBus` is `Clone` and can be shared across threads.
#[derive(Clone)]
pub struct EventBus {
    handlers: Arc<RwLock<HashMap<TypeId, Vec<Handler>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
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
    /// Returns immediately without waiting for handlers to complete.
    pub async fn emit<E: Send + Sync + 'static>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let handlers = self.handlers.read().await;
        if let Some(subs) = handlers.get(&type_id) {
            for handler in subs {
                let h = handler.clone();
                let e = event.clone();
                tokio::spawn(async move {
                    h(e).await;
                });
            }
        }
    }

    /// Emit an event and wait for all subscribers to complete.
    pub async fn emit_and_wait<E: Send + Sync + 'static>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        let event = Arc::new(event) as Arc<dyn Any + Send + Sync>;
        let handlers = self.handlers.read().await;
        if let Some(subs) = handlers.get(&type_id) {
            let mut tasks = Vec::new();
            for handler in subs {
                let h = handler.clone();
                let e = event.clone();
                tasks.push(tokio::spawn(async move {
                    h(e).await;
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestEvent {
        value: usize,
    }

    struct OtherEvent;

    #[tokio::test]
    async fn test_emit_and_subscribe() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let c = counter.clone();
        bus.subscribe(move |event: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(event.value, Ordering::SeqCst);
            }
        })
        .await;

        bus.emit_and_wait(TestEvent { value: 42 }).await;
        assert_eq!(counter.load(Ordering::SeqCst), 42);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = counter.clone();
            bus.subscribe(move |_: Arc<TestEvent>| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        }

        bus.emit_and_wait(TestEvent { value: 1 }).await;
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_no_cross_type_dispatch() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let c = counter.clone();
        bus.subscribe(move |_: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

        bus.emit_and_wait(OtherEvent).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
