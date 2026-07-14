use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use rdkafka::producer::FutureProducer;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{BackendState, PendingRequests};

use crate::config::KafkaConfig;

/// Shared inner state for `KafkaEventBus`, behind an `Arc`.
pub(crate) struct KafkaInner {
    pub config: KafkaConfig,
    pub producer: FutureProducer,
    pub state: Arc<BackendState>,
    /// Correlation map for in-flight `request` calls awaiting a reply
    /// (ReplyingKafkaTemplate pattern).
    pub pending: Arc<PendingRequests>,
    /// Lazily-started per-process reply consumer; holds its cancel token so
    /// shutdown can stop it. Started on the first `request`/`request_with`.
    pub reply_consumer: OnceCell<CancellationToken>,
    /// Cancel tokens for responder (request-topic) consumers, keyed by the
    /// request `TypeId`. One per `respond` registration.
    pub responder_cancels: std::sync::Mutex<HashMap<TypeId, CancellationToken>>,
    /// Sticky shutdown signal so requesters cannot miss cancellation between
    /// their initial shutdown check and the reply wait.
    pub request_cancel: CancellationToken,
    /// Per-bus-instance nonce identifying this instance's reply topic and reply
    /// consumer group. Minted once in the builder so two bus instances sharing a
    /// config in one process get disjoint reply topics AND groups.
    pub instance_id: u64,
    /// Cached per-instance reply topic (`<group-id>.replies.<instance-id-hex>`).
    /// Constant for the bus's lifetime, so it is formatted once at construction
    /// rather than re-`format!`ed on every request.
    pub reply_topic: String,
}
