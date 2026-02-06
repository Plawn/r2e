//! Backend trait and gRPC implementation for OpenFGA.

use crate::config::OpenFgaConfig;
use crate::error::OpenFgaError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Trait for OpenFGA backends.
///
/// This trait abstracts the OpenFGA API, allowing for different implementations
/// (gRPC, mock for testing, etc.).
pub trait OpenFgaBackend: Send + Sync + 'static {
    /// Check if a user has a relation to an object.
    ///
    /// Returns `Ok(true)` if allowed, `Ok(false)` if denied.
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>>;

    /// List all objects of a given type that a user has a relation to.
    fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, OpenFgaError>> + Send + '_>>;

    /// Write a relationship tuple (grant permission).
    fn write_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>>;

    /// Delete a relationship tuple (revoke permission).
    fn delete_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>>;
}

/// gRPC-based OpenFGA backend using `openfga-rs`.
pub struct GrpcBackend {
    client: Arc<
        tokio::sync::RwLock<
            openfga_rs::open_fga_service_client::OpenFgaServiceClient<tonic::transport::Channel>,
        >,
    >,
    store_id: String,
    model_id: Option<String>,
    #[allow(dead_code)]
    request_timeout: Duration,
}

impl GrpcBackend {
    /// Connect to an OpenFGA server with the given configuration.
    pub async fn connect(config: &OpenFgaConfig) -> Result<Self, OpenFgaError> {
        config.validate()?;

        let endpoint = tonic::transport::Endpoint::from_shared(config.endpoint.clone())
            .map_err(|e| OpenFgaError::InvalidConfig(e.to_string()))?
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .timeout(Duration::from_secs(config.request_timeout_secs));

        let channel = endpoint.connect().await?;

        let client = openfga_rs::open_fga_service_client::OpenFgaServiceClient::new(channel);

        Ok(Self {
            client: Arc::new(tokio::sync::RwLock::new(client)),
            store_id: config.store_id.clone(),
            model_id: config.model_id.clone(),
            request_timeout: Duration::from_secs(config.request_timeout_secs),
        })
    }

    fn make_check_tuple_key(
        user: &str,
        relation: &str,
        object: &str,
    ) -> openfga_rs::CheckRequestTupleKey {
        openfga_rs::CheckRequestTupleKey {
            user: user.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        }
    }
}

impl OpenFgaBackend for GrpcBackend {
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>> {
        let user = user.to_string();
        let relation = relation.to_string();
        let object = object.to_string();
        let store_id = self.store_id.clone();
        let model_id = self.model_id.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let request = openfga_rs::CheckRequest {
                store_id,
                authorization_model_id: model_id.unwrap_or_default(),
                tuple_key: Some(Self::make_check_tuple_key(&user, &relation, &object)),
                contextual_tuples: None,
                context: None,
                trace: false,
            };

            let mut client = client.write().await;
            let response = client.check(request).await?;
            Ok(response.into_inner().allowed)
        })
    }

    fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, OpenFgaError>> + Send + '_>> {
        let user = user.to_string();
        let relation = relation.to_string();
        let object_type = object_type.to_string();
        let store_id = self.store_id.clone();
        let model_id = self.model_id.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let request = openfga_rs::ListObjectsRequest {
                store_id,
                authorization_model_id: model_id.unwrap_or_default(),
                r#type: object_type,
                relation,
                user,
                contextual_tuples: None,
                context: None,
            };

            let mut client = client.write().await;
            let response = client.list_objects(request).await?;
            Ok(response.into_inner().objects)
        })
    }

    fn write_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        let user = user.to_string();
        let relation = relation.to_string();
        let object = object.to_string();
        let store_id = self.store_id.clone();
        let model_id = self.model_id.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let tuple = openfga_rs::TupleKey {
                user,
                relation,
                object,
                condition: None,
            };

            let request = openfga_rs::WriteRequest {
                store_id,
                authorization_model_id: model_id.unwrap_or_default(),
                writes: Some(openfga_rs::WriteRequestWrites {
                    tuple_keys: vec![tuple],
                }),
                deletes: None,
            };

            let mut client = client.write().await;
            client.write(request).await?;
            Ok(())
        })
    }

    fn delete_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        let user = user.to_string();
        let relation = relation.to_string();
        let object = object.to_string();
        let store_id = self.store_id.clone();
        let model_id = self.model_id.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let tuple = openfga_rs::TupleKeyWithoutCondition {
                user,
                relation,
                object,
            };

            let request = openfga_rs::WriteRequest {
                store_id,
                authorization_model_id: model_id.unwrap_or_default(),
                writes: None,
                deletes: Some(openfga_rs::WriteRequestDeletes {
                    tuple_keys: vec![tuple],
                }),
            };

            let mut client = client.write().await;
            client.write(request).await?;
            Ok(())
        })
    }
}

/// A mock backend for testing purposes.
///
/// Stores tuples in memory and performs simple checks.
pub struct MockBackend {
    tuples: Arc<dashmap::DashSet<(String, String, String)>>,
}

impl MockBackend {
    /// Create a new mock backend.
    pub fn new() -> Self {
        Self {
            tuples: Arc::new(dashmap::DashSet::new()),
        }
    }

    /// Add a tuple to the mock backend.
    pub fn add_tuple(&self, user: &str, relation: &str, object: &str) {
        self.tuples
            .insert((user.to_string(), relation.to_string(), object.to_string()));
    }

    /// Remove a tuple from the mock backend.
    pub fn remove_tuple(&self, user: &str, relation: &str, object: &str) {
        self.tuples
            .remove(&(user.to_string(), relation.to_string(), object.to_string()));
    }

    /// Check if a tuple exists in the mock backend.
    pub fn has_tuple(&self, user: &str, relation: &str, object: &str) -> bool {
        self.tuples
            .contains(&(user.to_string(), relation.to_string(), object.to_string()))
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
        let allowed = self.has_tuple(user, relation, object);
        Box::pin(std::future::ready(Ok(allowed)))
    }

    fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, OpenFgaError>> + Send + '_>> {
        let user = user.to_string();
        let relation = relation.to_string();
        let prefix = format!("{}:", object_type);

        let objects: Vec<String> = self
            .tuples
            .iter()
            .filter(|t| t.0 == user && t.1 == relation && t.2.starts_with(&prefix))
            .map(|t| t.2.clone())
            .collect();

        Box::pin(std::future::ready(Ok(objects)))
    }

    fn write_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        self.add_tuple(user, relation, object);
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        self.remove_tuple(user, relation, object);
        Box::pin(std::future::ready(Ok(())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_backend_check() {
        let backend = MockBackend::new();
        backend.add_tuple("user:alice", "viewer", "document:1");

        assert!(backend
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());
        assert!(!backend
            .check("user:bob", "viewer", "document:1")
            .await
            .unwrap());
        assert!(!backend
            .check("user:alice", "editor", "document:1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_mock_backend_list_objects() {
        let backend = MockBackend::new();
        backend.add_tuple("user:alice", "viewer", "document:1");
        backend.add_tuple("user:alice", "viewer", "document:2");
        backend.add_tuple("user:alice", "editor", "document:3");

        let objects = backend
            .list_objects("user:alice", "viewer", "document")
            .await
            .unwrap();
        assert_eq!(objects.len(), 2);
        assert!(objects.contains(&"document:1".to_string()));
        assert!(objects.contains(&"document:2".to_string()));
    }

    #[tokio::test]
    async fn test_mock_backend_write_delete() {
        let backend = MockBackend::new();

        assert!(!backend
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());

        backend
            .write_tuple("user:alice", "viewer", "document:1")
            .await
            .unwrap();
        assert!(backend
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());

        backend
            .delete_tuple("user:alice", "viewer", "document:1")
            .await
            .unwrap();
        assert!(!backend
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());
    }
}
