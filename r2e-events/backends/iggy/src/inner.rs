use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use iggy::prelude::{Identifier, IggyClient};
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{BackendState, PendingRequests};

use crate::config::IggyConfig;

/// Shared inner state for `IggyEventBus`, behind an `Arc`.
pub(crate) struct IggyInner {
    pub config: IggyConfig,
    pub client: Arc<IggyClient>,
    pub state: Arc<BackendState>,
    /// Per-bus-instance nonce (minted in the builder). Distinguishes two bus
    /// instances sharing a config within one process so their reply topics and
    /// standalone reply-consumer names never collide.
    pub instance_id: u64,
    /// Cached instance-private reply topic (`<consumer_group>.replies.<id-hex>`),
    /// derived once from `instance_id` instead of per request.
    pub reply_topic: String,
    /// Cached `Identifier` for the stream name — constant for the bus lifetime,
    /// avoids re-parsing on every publish.
    pub stream_id: Identifier,
    /// Cached `Identifier`s for topic names, populated on first use per topic.
    /// Avoids `Identifier::named()` parsing on every publish.
    pub topic_ids: RwLock<HashMap<Arc<str>, Identifier>>,
    /// Correlation map for in-flight request-reply calls (`request`/`respond`).
    pub pending: Arc<PendingRequests>,
    /// Sticky shutdown token so requesters awaiting a reply fail fast with
    /// [`EventBusError::Shutdown`](r2e_events::EventBusError::Shutdown) instead
    /// of waiting out their per-request timeout.
    pub request_cancel: CancellationToken,
    /// Cancellation tokens for the request-reply poller tasks.
    pub rr_cancels: Mutex<RequestReplyCancels>,
}

/// Cancellation handles for the request-reply pollers.
///
/// The reply poller is started lazily on the first `request` call (one per
/// process); responder pollers are started per `respond` registration.
#[derive(Default)]
pub(crate) struct RequestReplyCancels {
    /// The single per-process reply poller (`None` until the first request).
    pub reply_poller: Option<CancellationToken>,
    /// One responder poller per registered request type.
    pub responder_pollers: Vec<CancellationToken>,
}
