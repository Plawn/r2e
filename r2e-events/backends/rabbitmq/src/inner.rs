use std::sync::Arc;

use r2e_events::backend::BackendState;

use crate::config::RabbitMqConfig;

/// Shared inner state for `RabbitMqEventBus`, behind an `Arc`.
pub(crate) struct RabbitMqInner {
    pub config: RabbitMqConfig,
    pub channel: lapin::Channel,
    pub state: Arc<BackendState>,
}
