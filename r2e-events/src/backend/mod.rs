//! Shared utilities for distributed event bus backends.
//!
//! This module provides building blocks that all distributed backends
//! (Iggy, Kafka, Pulsar, RabbitMQ) share: topic registries, type-erased
//! dispatch types, metadata header encoding, and common inner state.

mod dispatch;
mod metadata_codec;
mod state;
mod topic;

pub use dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
pub use metadata_codec::{
    decode_metadata, encode_metadata, HEADER_CORRELATION_ID, HEADER_EVENT_ID,
    HEADER_PARTITION_KEY, HEADER_TIMESTAMP, HEADER_USER_PREFIX,
};
pub use state::{BackendState, InFlightGuard, LocallyDispatchedSet, DEFAULT_BACKEND_CONCURRENCY};
pub use topic::{sanitize_topic_name, TopicRegistry};
