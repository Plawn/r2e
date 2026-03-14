use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::Arc;

use iggy::prelude::IggyClient;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::IggyConfig;
use crate::dispatch::TopicHandlers;
use crate::topic::TopicRegistry;

/// Shared inner state for `IggyEventBus`, behind an `Arc`.
pub(crate) struct IggyInner {
    pub config: IggyConfig,
    pub client: Arc<IggyClient>,
    pub shutdown: AtomicBool,
    pub next_id: AtomicU64,
    /// Per-TypeId handler registry.
    pub handlers: RwLock<HashMap<TypeId, TopicHandlers>>,
    /// TypeId → resolved topic name.
    pub topic_registry: RwLock<TopicRegistry>,
    /// Cancellation tokens for background pollers, keyed by TypeId.
    pub poller_cancels: Mutex<HashMap<TypeId, CancellationToken>>,
    /// Number of handlers currently executing.
    pub in_flight: AtomicUsize,
    /// Notified when `in_flight` drops to zero.
    pub in_flight_zero: Notify,
    /// Set of topics we've already ensured exist in Iggy.
    pub ensured_topics: Mutex<HashSet<String>>,
}
