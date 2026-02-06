//! OpenFGA registry - clonable handle to the OpenFGA backend.

use crate::backend::{GrpcBackend, MockBackend, OpenFgaBackend};
use crate::cache::{CacheKey, DecisionCache};
use crate::config::OpenFgaConfig;
use crate::error::OpenFgaError;
use std::sync::Arc;

/// Clonable handle to an OpenFGA backend with optional decision caching.
///
/// This is the main entry point for OpenFGA operations. It wraps the backend
/// and provides caching for authorization checks.
///
/// # Examples
///
/// ```ignore
/// use r2e_openfga::{OpenFgaConfig, OpenFgaRegistry};
///
/// let config = OpenFgaConfig::new("http://localhost:8080", "store-id")
///     .with_cache(true, 60);
///
/// let registry = OpenFgaRegistry::connect(config).await?;
///
/// // Check authorization
/// let allowed = registry.check("user:alice", "viewer", "document:1").await?;
/// ```
#[derive(Clone)]
pub struct OpenFgaRegistry {
    backend: Arc<dyn OpenFgaBackend>,
    cache: Option<Arc<DecisionCache>>,
}

impl OpenFgaRegistry {
    /// Create a new registry with a custom backend.
    pub fn new(backend: impl OpenFgaBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            cache: None,
        }
    }

    /// Create a new registry with caching enabled.
    pub fn with_cache(backend: impl OpenFgaBackend, cache_ttl_secs: u64) -> Self {
        Self {
            backend: Arc::new(backend),
            cache: Some(Arc::new(DecisionCache::new(cache_ttl_secs))),
        }
    }

    /// Connect to an OpenFGA server using gRPC.
    pub async fn connect(config: OpenFgaConfig) -> Result<Self, OpenFgaError> {
        let backend = GrpcBackend::connect(&config).await?;

        let cache = if config.cache_enabled {
            Some(Arc::new(DecisionCache::new(config.cache_ttl_secs)))
        } else {
            None
        };

        Ok(Self {
            backend: Arc::new(backend),
            cache,
        })
    }

    /// Create a registry with a mock backend for testing.
    pub fn mock() -> (Self, Arc<MockBackend>) {
        let backend = Arc::new(MockBackend::new());
        let registry = Self {
            backend: backend.clone(),
            cache: None,
        };
        (registry, backend)
    }

    /// Create a registry with a mock backend and caching enabled.
    pub fn mock_with_cache(cache_ttl_secs: u64) -> (Self, Arc<MockBackend>) {
        let backend = Arc::new(MockBackend::new());
        let registry = Self {
            backend: backend.clone(),
            cache: Some(Arc::new(DecisionCache::new(cache_ttl_secs))),
        };
        (registry, backend)
    }

    /// Check if a user has a relation to an object.
    ///
    /// Returns `Ok(true)` if allowed, `Ok(false)` if denied.
    ///
    /// Results are cached if caching is enabled.
    pub async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, OpenFgaError> {
        // Check cache first
        if let Some(cache) = &self.cache {
            let key = CacheKey::new(user, relation, object);
            if let Some(cached) = cache.get(&key) {
                tracing::trace!(user, relation, object, allowed = cached, "cache hit");
                return Ok(cached);
            }
        }

        // Query backend
        let allowed = self.backend.check(user, relation, object).await?;
        tracing::trace!(user, relation, object, allowed, "authorization check");

        // Store in cache
        if let Some(cache) = &self.cache {
            let key = CacheKey::new(user, relation, object);
            cache.set(key, allowed);
        }

        Ok(allowed)
    }

    /// List all objects of a given type that a user has a relation to.
    ///
    /// This operation is not cached.
    pub async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Result<Vec<String>, OpenFgaError> {
        self.backend.list_objects(user, relation, object_type).await
    }

    /// Write a relationship tuple (grant permission).
    ///
    /// Invalidates cached decisions for the object.
    pub async fn write_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<(), OpenFgaError> {
        self.backend.write_tuple(user, relation, object).await?;

        // Invalidate cache for this object
        if let Some(cache) = &self.cache {
            cache.invalidate_object(object);
        }

        tracing::debug!(user, relation, object, "wrote tuple");
        Ok(())
    }

    /// Delete a relationship tuple (revoke permission).
    ///
    /// Invalidates cached decisions for the object.
    pub async fn delete_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<(), OpenFgaError> {
        self.backend.delete_tuple(user, relation, object).await?;

        // Invalidate cache for this object
        if let Some(cache) = &self.cache {
            cache.invalidate_object(object);
        }

        tracing::debug!(user, relation, object, "deleted tuple");
        Ok(())
    }

    /// Invalidate all cached decisions for an object.
    pub fn invalidate_object(&self, object: &str) {
        if let Some(cache) = &self.cache {
            cache.invalidate_object(object);
        }
    }

    /// Invalidate all cached decisions for a user.
    pub fn invalidate_user(&self, user: &str) {
        if let Some(cache) = &self.cache {
            cache.invalidate_user(user);
        }
    }

    /// Clear all cached decisions.
    pub fn clear_cache(&self) {
        if let Some(cache) = &self.cache {
            cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_check_with_mock() {
        let (registry, backend) = OpenFgaRegistry::mock();
        backend.add_tuple("user:alice", "viewer", "document:1");

        assert!(registry.check("user:alice", "viewer", "document:1").await.unwrap());
        assert!(!registry.check("user:bob", "viewer", "document:1").await.unwrap());
    }

    #[tokio::test]
    async fn test_registry_write_invalidates_cache() {
        let (registry, _backend) = OpenFgaRegistry::mock_with_cache(60);

        // Check returns false, gets cached
        assert!(!registry.check("user:alice", "viewer", "document:1").await.unwrap());

        // Write tuple and invalidate cache
        registry.write_tuple("user:alice", "viewer", "document:1").await.unwrap();

        // Check should now return true (cache was invalidated)
        assert!(registry.check("user:alice", "viewer", "document:1").await.unwrap());
    }
}
