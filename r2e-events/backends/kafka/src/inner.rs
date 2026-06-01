use std::sync::Arc;

use rdkafka::producer::FutureProducer;

use r2e_events::backend::BackendState;

use crate::config::KafkaConfig;

/// Shared inner state for `KafkaEventBus`, behind an `Arc`.
pub(crate) struct KafkaInner {
    pub config: KafkaConfig,
    pub producer: FutureProducer,
    pub state: Arc<BackendState>,
}
