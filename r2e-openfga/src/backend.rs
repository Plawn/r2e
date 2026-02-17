//! Backend trait and implementations for OpenFGA authorization.
//!
//! [`OpenFgaBackend`] is the core abstraction — implement it to plug in a
//! custom authorization check (REST proxy, in-process evaluation, etc.).
//!
//! Provided implementations:
//! - [`GrpcBackend`] — production gRPC client wrapping `openfga-rs`
//! - [`MockBackend`] — in-memory mock for tests

use crate::config::OpenFgaConfig;
use crate::error::OpenFgaError;
use openfga_rs::open_fga_service_client::OpenFgaServiceClient;
use openfga_rs::tonic;
use openfga_rs::{CheckRequest, CheckRequestTupleKey};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tonic::transport::Channel;

/// Backend trait for OpenFGA authorization checks.
///
/// Only `check` is required — that's the operation the registry caches
/// and the guard delegates to. For writes, deletes, list objects, model
/// management, etc., use the concrete backend directly (e.g.,
/// [`GrpcBackend::client()`] for the raw gRPC client).
pub trait OpenFgaBackend: Send + Sync + 'static {
    /// Check if `user` has `relation` to `object`.
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>>;
}

// ── GrpcBackend ────────────────────────────────────────────────────────

/// Production gRPC backend wrapping the `openfga-rs` client.
///
/// Use [`client()`](Self::client) for raw access to the full OpenFGA API
/// (batch writes, list objects, model management, etc.).
///
/// The tonic client is cheap to clone (shares the underlying HTTP/2 channel).
///
/// # Example
///
/// ```ignore
/// use r2e_openfga::{OpenFgaConfig, GrpcBackend};
/// use openfga_rs::{ListObjectsRequest, TupleKey, WriteRequest, WriteRequestWrites};
///
/// let config = OpenFgaConfig::new("http://localhost:8080", "store-id");
/// let backend = GrpcBackend::connect(&config).await?;
///
/// // Write a tuple via raw client
/// let mut client = backend.client().clone();
/// client.write(tonic::Request::new(WriteRequest {
///     store_id: backend.store_id().to_string(),
///     writes: Some(WriteRequestWrites {
///         tuple_keys: vec![TupleKey {
///             user: "user:alice".into(),
///             relation: "viewer".into(),
///             object: "document:1".into(),
///             condition: None,
///         }],
///     }),
///     ..Default::default()
/// })).await?;
/// ```
#[derive(Clone)]
pub struct GrpcBackend {
    client: OpenFgaServiceClient<Channel>,
    store_id: String,
    model_id: Option<String>,
    api_token: Option<String>,
}

impl GrpcBackend {
    /// Connect to an OpenFGA server using the given config.
    pub async fn connect(config: &OpenFgaConfig) -> Result<Self, OpenFgaError> {
        config.validate()?;

        let endpoint = tonic::transport::Endpoint::from_shared(config.endpoint.clone())
            .map_err(|e| OpenFgaError::ConnectionFailed(e.to_string()))?
            .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs));

        let channel = endpoint.connect().await?;

        Ok(Self {
            client: OpenFgaServiceClient::new(channel),
            store_id: config.store_id.clone(),
            model_id: config.model_id.clone(),
            api_token: config.api_token.clone(),
        })
    }

    /// Returns a reference to the raw gRPC client.
    ///
    /// Clone it before calling methods (tonic clients are cheap to clone).
    pub fn client(&self) -> &OpenFgaServiceClient<Channel> {
        &self.client
    }

    /// Returns the store ID.
    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    /// Returns the authorization model ID, if configured.
    pub fn model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    /// Build a `tonic::Request`, injecting the Bearer token if configured.
    fn make_request<T>(&self, msg: T) -> Result<tonic::Request<T>, OpenFgaError> {
        let mut request = tonic::Request::new(msg);
        if let Some(token) = &self.api_token {
            request.metadata_mut().insert(
                "authorization",
                format!("Bearer {}", token)
                    .parse()
                    .map_err(|e: tonic::metadata::errors::InvalidMetadataValue| {
                        OpenFgaError::InvalidConfig(format!(
                            "invalid api_token for header: {}",
                            e
                        ))
                    })?,
            );
        }
        Ok(request)
    }
}

impl OpenFgaBackend for GrpcBackend {
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>> {
        let req = CheckRequest {
            store_id: self.store_id.clone(),
            authorization_model_id: self.model_id.clone().unwrap_or_default(),
            tuple_key: Some(CheckRequestTupleKey {
                user: user.to_string(),
                relation: relation.to_string(),
                object: object.to_string(),
            }),
            ..Default::default()
        };

        Box::pin(async move {
            let request = self.make_request(req)?;
            let resp = self
                .client
                .clone()
                .check(request)
                .await
                .map_err(OpenFgaError::from)?;
            Ok(resp.into_inner().allowed)
        })
    }
}

// ── MockBackend ────────────────────────────────────────────────────────

/// In-memory mock backend for testing.
///
/// Stores tuples as `(user, relation, object)` triples in a `DashSet`.
/// Only performs direct tuple lookups — does **not** model transitive
/// relationships like a real OpenFGA server would.
///
/// # Example
///
/// ```ignore
/// use r2e_openfga::OpenFgaRegistry;
///
/// let (registry, mock) = OpenFgaRegistry::mock();
/// mock.add_tuple("user:alice", "viewer", "document:1");
///
/// assert!(registry.check("user:alice", "viewer", "document:1").await.unwrap());
/// assert!(!registry.check("user:bob", "viewer", "document:1").await.unwrap());
/// ```
pub struct MockBackend {
    tuples: Arc<dashmap::DashSet<(String, String, String)>>,
}

impl MockBackend {
    /// Create a new empty mock backend.
    pub fn new() -> Self {
        Self {
            tuples: Arc::new(dashmap::DashSet::new()),
        }
    }

    /// Add a relationship tuple.
    pub fn add_tuple(&self, user: &str, relation: &str, object: &str) {
        self.tuples
            .insert((user.to_string(), relation.to_string(), object.to_string()));
    }

    /// Remove a relationship tuple.
    pub fn remove_tuple(&self, user: &str, relation: &str, object: &str) {
        self.tuples
            .remove(&(user.to_string(), relation.to_string(), object.to_string()));
    }

    /// Check if a tuple exists (direct lookup only, no transitive evaluation).
    pub fn has_tuple(&self, user: &str, relation: &str, object: &str) -> bool {
        self.tuples
            .contains(&(user.to_string(), relation.to_string(), object.to_string()))
    }

    /// List all objects of a given type that a user has a relation to.
    pub fn list_objects(&self, user: &str, relation: &str, object_type: &str) -> Vec<String> {
        let prefix = format!("{}:", object_type);
        self.tuples
            .iter()
            .filter(|t| t.0 == user && t.1 == relation && t.2.starts_with(&prefix))
            .map(|t| t.2.clone())
            .collect()
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenFgaBackend for MockBackend {
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>> {
        let result = self.has_tuple(user, relation, object);
        Box::pin(async move { Ok(result) })
    }
}
