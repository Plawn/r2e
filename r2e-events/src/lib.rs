use std::future::Future;
use std::sync::Arc;

mod local;

pub use local::{LocalEventBus, DEFAULT_MAX_CONCURRENCY};

/// Pluggable event bus trait — implement for custom backends (Kafka, Redis, etc.).
///
/// All methods are generic and monomorphized at compile time. No dynamic
/// dispatch. Serde bounds are enforced so backends can serialize events
/// for remote transport. In-memory backends (like [`LocalEventBus`]) are
/// free to ignore serialization — the bounds exist only for trait compatibility.
///
/// # Example
///
/// ```ignore
/// #[derive(Clone)]
/// pub struct KafkaEventBus { /* ... */ }
///
/// impl EventBus for KafkaEventBus {
///     fn subscribe<E, F, Fut>(&self, handler: F) -> impl Future<Output = ()> + Send
///     where
///         E: serde::de::DeserializeOwned + Send + Sync + 'static,
///         F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
///         Fut: Future<Output = ()> + Send + 'static,
///     {
///         async move { /* register handler for Kafka topic */ }
///     }
///     // ...
/// }
/// ```
pub trait EventBus: Clone + Send + Sync + 'static {
    /// Subscribe to events of type `E`.
    ///
    /// The handler receives `Arc<E>` and is called for every `emit()` of that type.
    fn subscribe<E, F, Fut>(&self, handler: F) -> impl Future<Output = ()> + Send
    where
        E: serde::de::DeserializeOwned + Send + Sync + 'static,
        F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static;

    /// Emit an event, spawning all subscribers as concurrent tasks.
    ///
    /// Returns after all handlers have been spawned (not necessarily completed).
    fn emit<E>(&self, event: E) -> impl Future<Output = ()> + Send
    where
        E: serde::Serialize + Send + Sync + 'static;

    /// Emit an event and wait for all subscribers to complete.
    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = ()> + Send
    where
        E: serde::Serialize + Send + Sync + 'static;

    /// Remove all registered event handlers.
    fn clear(&self) -> impl Future<Output = ()> + Send;
}

pub mod prelude {
    //! Re-exports of the most commonly used event types.
    pub use crate::{EventBus, LocalEventBus};
}
