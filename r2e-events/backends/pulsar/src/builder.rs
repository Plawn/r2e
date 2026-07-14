use std::collections::HashMap;
use std::sync::Arc;

use pulsar::{Authentication, Pulsar, TokioExecutor};
use tokio::sync::Mutex;

use r2e_events::backend::{instance_id, BackendState, PendingRequests, TopicRegistry};
use r2e_events::{DlqPublisher, EventBusError};

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

    /// Register an event type using its [`Event::topic()`] name.
    pub fn register_event<E: r2e_events::Event + 'static>(self) -> Self {
        self.topic::<E>(E::topic())
    }

    /// Connect to the Pulsar cluster and return a ready-to-use [`PulsarEventBus`].
    pub async fn connect(self) -> Result<PulsarEventBus, EventBusError> {
        let mut builder = Pulsar::builder(&self.config.service_url, TokioExecutor);

        // Configure JWT authentication if a token is provided
        if let Some(ref token) = self.config.auth_token {
            builder = builder.with_auth(Authentication {
                name: "token".to_string(),
                data: token.as_bytes().to_vec(),
            });
        }

        builder =
            builder.with_tls_hostname_verification_enabled(self.config.tls_hostname_verification);

        let pulsar = builder.build().await.map_err(map_pulsar_error)?;

        // Mint one instance nonce per bus. The instance-private reply topic is
        // derived from it (and cached) on first request-reply use.
        let instance = instance_id();

        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<PulsarInner>| {
            let weak = weak.clone();
            let dlq: DlqPublisher = Arc::new(move |topic, payload, metadata| {
                let weak = weak.clone();
                Box::pin(async move {
                    let inner = weak.upgrade().ok_or(EventBusError::Shutdown)?;
                    PulsarEventBus { inner }
                        .publish(&topic, payload, &metadata)
                        .await
                })
            });
            PulsarInner {
                config: self.config,
                pulsar,
                producers: Mutex::new(HashMap::new()),
                state: Arc::new(BackendState::with_dlq_publisher(
                    self.topic_registry,
                    Some(dlq),
                )),
                full_topics: std::sync::RwLock::new(HashMap::new()),
                instance_id: instance,
                reply_topic_full: std::sync::OnceLock::new(),
                pending: Arc::new(PendingRequests::new()),
                reply_consumer: std::sync::OnceLock::new(),
                responder_cancels: std::sync::Mutex::new(HashMap::new()),
            }
        });

        Ok(PulsarEventBus { inner })
    }
}
