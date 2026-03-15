use std::sync::Arc;

use iggy::prelude::*;

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::EventBusError;

use crate::bus::IggyEventBus;
use crate::config::{IggyConfig, Transport};
use crate::error::map_iggy_error;
use crate::inner::IggyInner;

/// Builder for [`IggyEventBus`].
///
/// # Example
///
/// ```ignore
/// let bus = IggyEventBus::builder(config)
///     .topic::<UserCreated>("user-created")
///     .topic::<OrderPlaced>("order-placed")
///     .connect()
///     .await?;
/// ```
pub struct IggyEventBusBuilder {
    config: IggyConfig,
    topic_registry: TopicRegistry,
}

impl IggyEventBusBuilder {
    pub(crate) fn new(config: IggyConfig) -> Self {
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

    /// Connect to the Iggy server and return a ready-to-use [`IggyEventBus`].
    pub async fn connect(self) -> Result<IggyEventBus, EventBusError> {
        let client = build_client(&self.config).map_err(map_iggy_error)?;

        client.connect().await.map_err(map_iggy_error)?;

        // Authenticate if credentials provided
        let username = self
            .config
            .username
            .as_deref()
            .unwrap_or(DEFAULT_ROOT_USERNAME);
        let password = self
            .config
            .password
            .as_deref()
            .unwrap_or(DEFAULT_ROOT_PASSWORD);
        client
            .login_user(username, password)
            .await
            .map_err(map_iggy_error)?;

        // Ensure stream exists if auto_create is on
        if self.config.auto_create {
            ensure_stream(&client, &self.config.stream_name).await?;
        }

        let inner = IggyInner {
            config: self.config,
            client: Arc::new(client),
            state: Arc::new(BackendState::new(self.topic_registry)),
        };

        Ok(IggyEventBus {
            inner: Arc::new(inner),
        })
    }
}

fn build_client(config: &IggyConfig) -> Result<IggyClient, IggyError> {
    match config.transport {
        Transport::Tcp => IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(config.address.clone())
            .build(),
        Transport::Quic => IggyClientBuilder::new()
            .with_quic()
            .with_server_address(config.address.clone())
            .build(),
        Transport::Http => IggyClientBuilder::new()
            .with_http()
            .build(),
    }
}

async fn ensure_stream(client: &IggyClient, stream_name: &str) -> Result<(), EventBusError> {
    match client.create_stream(stream_name).await {
        Ok(_) => {
            tracing::info!(stream = %stream_name, "created Iggy stream");
            Ok(())
        }
        Err(_) => {
            // Stream likely already exists — verify
            client
                .get_stream(&Identifier::named(stream_name).map_err(map_iggy_error)?)
                .await
                .map_err(map_iggy_error)?;
            Ok(())
        }
    }
}
