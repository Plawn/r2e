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
    /// Cached producers per topic. `Producer` is not `Send` across tasks,
    /// so we guard with a `Mutex` and reuse per topic name.
    pub producers: Mutex<HashMap<String, Producer<TokioExecutor>>>,
    pub state: Arc<BackendState>,
}
