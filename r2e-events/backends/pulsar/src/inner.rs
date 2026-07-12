use std::collections::HashMap;
use std::sync::Arc;

use pulsar::{producer::Producer, Pulsar, TokioExecutor};
use tokio::sync::Mutex;

use r2e_events::backend::BackendState;

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
}
