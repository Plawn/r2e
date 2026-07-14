//! Shared utilities for distributed event bus backends.
//!
//! This module provides building blocks that all distributed backends
//! (Iggy, Kafka, Pulsar, RabbitMQ) share: topic registries, type-erased
//! dispatch types, metadata header encoding, request-reply plumbing
//! (correlation map + responder registry), and common inner state.

mod dispatch;
mod metadata_codec;
mod pending;
mod reconnect;
mod state;
mod topic;
mod watermark;

pub use dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
pub use metadata_codec::{
    decode_metadata, decode_reply_headers, encode_metadata, encode_reply_headers, HeaderPair,
    ReplyHeaders, HEADER_CORRELATION_ID, HEADER_EVENT_ID, HEADER_PARTITION_KEY, HEADER_REPLY_ERROR,
    HEADER_REPLY_TO, HEADER_REQUEST_ID, HEADER_TIMESTAMP, HEADER_USER_PREFIX,
};
pub use pending::{await_reply, PendingGuard, PendingRequests, ReplyResult};
pub use reconnect::reconnect_loop;
pub use state::{
    spawn_completion_forwarder, BackendState, DispatchCompletion, DispatchOutcome, InFlightGuard,
    ResponderFn, COMPLETION_CHANNEL_CAPACITY, COMPLETION_DRAIN_TIMEOUT,
    DEFAULT_BACKEND_CONCURRENCY,
};
pub use topic::{
    instance_id, reply_topic, request_topic, responder_group, sanitize_topic_name, TopicRegistry,
    REQUEST_TOPIC_SUFFIX,
};
pub use watermark::WatermarkTracker;

/// The per-process identity that prefixes every `event_id` on this instance.
///
/// Exposed so distributed backends can build per-process consumer identities.
pub fn process_id() -> u64 {
    crate::process_id()
}
