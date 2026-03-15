use std::collections::HashMap;
use std::sync::Arc;

use pulsar::{Authentication, Pulsar, TokioExecutor};
use tokio::sync::Mutex;

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::EventBusError;

use crate::bus::PulsarEventBus;
use crate::config::PulsarConfig;
use crate::error::map_pulsar_error;
use crate::inner::PulsarInner;

/// Builder for [`PulsarEventBus`].
///
/// # Example
///
/// ```ignore
/// let bus = PulsarEventBus::builder(config)
///     .topic::<UserCreated>("user-created")
///     .topic::<OrderPlaced>("order-placed")
///     .connect()
///     .await?;
/// ```
pub struct PulsarEventBusBuilder {
    config: PulsarConfig,
    topic_registry: TopicRegistry,
}

impl PulsarEventBusBuilder {
    pub(crate) fn new(config: PulsarConfig) -> Self {
        Self {
            config,
            topic_registry: TopicRegistry::default(),
        }
    }

    /// Register an explicit topic name for event type `E`.
    pub fn topic<E: 'static>(mut self, name: impl Into<String>) -> Self {
        self.topic_registry.register::<E>(name);
        self
    }

    /// Connect to the Pulsar cluster and return a ready-to-use [`PulsarEventBus`].
    pub async fn connect(self) -> Result<PulsarEventBus, EventBusError> {
        let mut builder =
            Pulsar::builder(&self.config.service_url, TokioExecutor);

        // Configure JWT authentication if a token is provided
        if let Some(ref token) = self.config.auth_token {
            builder = builder.with_auth(Authentication {
                name: "token".to_string(),
                data: token.as_bytes().to_vec(),
            });
        }

        let pulsar = builder.build().await.map_err(map_pulsar_error)?;

        let inner = PulsarInner {
            config: self.config,
            pulsar,
            producers: Mutex::new(HashMap::new()),
            state: Arc::new(BackendState::new(self.topic_registry)),
        };

        Ok(PulsarEventBus {
            inner: Arc::new(inner),
        })
    }
}
