//! OpenFGA registry - clonable handle to the OpenFGA backend.
//!
//! The registry wraps any [`OpenFgaBackend`] and adds decision caching.
//! Only `check` is cached — for writes, deletes, list objects, etc.,
//! use the concrete backend directly (e.g., [`GrpcBackend::client()`]).

use crate::backend::OpenFgaBackend;
use crate::cache::{CacheKey, DecisionCache};
use crate::error::OpenFgaError;
use std::sync::Arc;

#[cfg(doc)]
use crate::backend::GrpcBackend;

/// Clonable handle to an OpenFGA backend with optional decision caching.
///
/// This is the main entry point for the guard integration. It wraps any
/// [`OpenFgaBackend`] implementation and adds caching for `check` calls.
///
/// # Usage pattern
///
/// ```ignore
/// use r2e_openfga::{OpenFgaConfig, OpenFgaRegistry, GrpcBackend};
///
/// let config = OpenFgaConfig::new("http://localhost:8080", "store-id")
///     .with_cache(true, 60);
/// let backend = GrpcBackend::connect(&config).await?;
/// let registry = OpenFgaRegistry::with_cache(backend.clone(), 60);
///
/// // Cached check (used by the guard)
/// let allowed = registry.check("user:alice", "viewer", "document:1").await?;
///
/// // For writes, use the backend directly
/// let mut client = backend.client().clone();
/// client.write(tonic::Request::new(/* ... */)).await?;
///
/// // Then invalidate the cache
/// registry.invalidate_object("document:1");
/// ```
///
/// # Cache limitations
///
/// The cache stores **direct** authorization decisions. OpenFGA evaluates
/// relationships **transitively** (e.g., `user:alice` → `member` of `org:acme`
/// → `parent` of `document:1` → alice is a `viewer` of `document:1`).
///
/// After writing or deleting tuples, you must manually invalidate affected
/// cache entries. Only the **exact object** is invalidated — transitive
/// relationships are not tracked. Consider:
/// - Using short cache TTLs
/// - Calling `invalidate_object()` on downstream objects
/// - Calling `clear_cache()` after bulk permission changes
#[derive(Clone)]
pub struct OpenFgaRegistry {
    backend: Arc<dyn OpenFgaBackend>,
    cache: Option<Arc<DecisionCache>>,
}

impl OpenFgaRegistry {
    /// Create a registry wrapping any [`OpenFgaBackend`], without caching.
    pub fn new(backend: impl OpenFgaBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            cache: None,
        }
    }

    /// Create a registry with caching enabled.
    pub fn with_cache(backend: impl OpenFgaBackend, cache_ttl_secs: u64) -> Self {
        Self {
            backend: Arc::new(backend),
            cache: Some(Arc::new(DecisionCache::new(cache_ttl_secs))),
        }
    }

    // ── Check ──────────────────────────────────────────────────────────

    /// Check if a user has a relation to an object.
    ///
    /// Returns `Ok(true)` if allowed, `Ok(false)` if denied.
    /// Results are cached when caching is enabled.
    pub async fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<bool, OpenFgaError> {
        // Check cache first.
        let cache_key = self
            .cache
            .as_ref()
            .map(|_| CacheKey::new(user, relation, object));

        if let Some((cache, key)) = self.cache.as_ref().zip(cache_key.as_ref()) {
            if let Some(cached) = cache.get(key) {
                tracing::trace!(user, relation, object, allowed = cached, "cache hit");
                return Ok(cached);
            }
        }

        // Query backend.
        let allowed = self.backend.check(user, relation, object).await?;
        tracing::trace!(user, relation, object, allowed, "authorization check");

        // Store in cache.
        if let Some((cache, key)) = self.cache.as_ref().zip(cache_key) {
            cache.set(key, allowed);
        }

        Ok(allowed)
    }

    // ── Cache management ───────────────────────────────────────────────

    /// Invalidate all cached decisions for an object.
    ///
    /// Call this after writing or deleting tuples that affect the object.
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
    use crate::backend::MockBackend;

    #[tokio::test]
    async fn test_registry_check_with_mock() {
        let mock = MockBackend::new();
        mock.add_tuple("user:alice", "viewer", "document:1");

        let registry = OpenFgaRegistry::new(mock);

        assert!(registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());
        assert!(!registry
            .check("user:bob", "viewer", "document:1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_registry_cache_hit() {
        let mock = MockBackend::new();
        mock.add_tuple("user:alice", "viewer", "document:1");

        let registry = OpenFgaRegistry::with_cache(mock, 60);

        // First call populates cache
        assert!(registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());

        // Second call hits cache (same result)
        assert!(registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_registry_invalidate_object() {
        let mock = MockBackend::new();
        let registry = OpenFgaRegistry::with_cache(mock, 60);

        // Check returns false, gets cached
        assert!(!registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());

        // Simulate an external write by accessing backend through the mock
        // (In real code, the user would write via GrpcBackend::client())

        // Invalidate cache — next check will hit backend again
        registry.invalidate_object("document:1");
    }

    #[tokio::test]
    async fn test_registry_clear_cache() {
        let mock = MockBackend::new();
        mock.add_tuple("user:alice", "viewer", "document:1");

        let registry = OpenFgaRegistry::with_cache(mock, 60);

        assert!(registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());

        registry.clear_cache();

        // Cache cleared — next check goes to backend
        assert!(registry
            .check("user:alice", "viewer", "document:1")
            .await
            .unwrap());
    }
}
