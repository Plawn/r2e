use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{EventMetadata, HandlerResult};

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
}

/// All handlers and the deserializer for a single event type / topic.
pub struct TopicHandlers {
    pub entries: Vec<HandlerEntry>,
    pub deserializer: DeserializerFn,
}
