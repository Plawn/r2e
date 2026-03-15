use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{EventFilter, EventMetadata, HandlerResult, RetryPolicy};

/// Type-erased async handler function.
pub type Handler = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>, EventMetadata) -> Pin<Box<dyn Future<Output = HandlerResult> + Send>>
        + Send
        + Sync,
>;

/// Type-erased deserializer: bytes -> `Arc<dyn Any + Send + Sync>`.
pub type DeserializerFn =
    Arc<dyn Fn(&[u8]) -> Result<Arc<dyn Any + Send + Sync>, String> + Send + Sync>;

/// A single registered handler with a unique ID.
pub struct HandlerEntry {
    pub id: u64,
    pub handler: Handler,
    /// Optional filter predicate — when set, the handler is skipped if the
    /// filter returns `false` for the event's metadata.
    pub filter: Option<EventFilter>,
    /// Optional retry policy — when set, failed handlers are retried
    /// according to this policy before being sent to the DLQ.
    pub retry_policy: Option<RetryPolicy>,
}

/// All handlers and the deserializer for a single event type / topic.
pub struct TopicHandlers {
    pub entries: Vec<HandlerEntry>,
    pub deserializer: DeserializerFn,
}
