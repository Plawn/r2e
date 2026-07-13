use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use pulsar::{producer::Producer, Pulsar, TokioExecutor};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{BackendState, PendingRequests};

use crate::config::PulsarConfig;

/// Shared inner state for `PulsarEventBus`, behind an `Arc`.
pub(crate) struct PulsarInner {
    pub config: PulsarConfig,
    pub pulsar: Pulsar<TokioExecutor>,
    /// Cached producers per topic. The outer `Mutex` guards only the map and is
    /// held briefly to clone a per-topic `Arc<Mutex<Producer>>`; the inner mutex
    /// serializes sends on that one topic. This keeps emits on distinct topics
    /// (and a first-emit broker connect) from blocking one another.
    pub producers: Mutex<HashMap<String, Arc<Mutex<Producer<TokioExecutor>>>>>,
    pub state: Arc<BackendState>,
    /// Per-bus-instance nonce identifying this bus's private reply topic. Minted
    /// once at build time (never the process id): two bus instances sharing a
    /// `config.subscription` in one process must not derive the same reply topic,
    /// or the second `Exclusive` reply subscription would fatally conflict.
    pub instance_id: u64,
    /// Fully-qualified, instance-private reply topic, derived once from
    /// `instance_id` on first use and reused for every request (both as the
    /// requester's reply-to header and the reply consumer's subscribed topic).
    pub reply_topic_full: OnceLock<String>,
    /// Requester-side correlation map: request id → the waiting caller's reply
    /// channel. Populated by `request_with`, drained by the reply consumer.
    pub pending: Arc<PendingRequests>,
    /// Cancellation token for the lazily-started, per-instance reply consumer.
    /// Set exactly once on the first `request_with`; cancelled on shutdown.
    pub reply_consumer: OnceLock<CancellationToken>,
    /// Cancellation tokens for responder (request-topic) consumers, keyed by
    /// request `TypeId`. Short, await-free critical sections → a std mutex.
    pub responder_cancels: std::sync::Mutex<HashMap<TypeId, CancellationToken>>,
}
