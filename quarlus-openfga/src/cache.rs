//! Decision cache for OpenFGA authorization checks.

use dashmap::DashMap;
use std::time::{Duration, Instant};

/// A cache key for authorization decisions.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub user: String,
    pub relation: String,
    pub object: String,
}

impl CacheKey {
    pub fn new(user: &str, relation: &str, object: &str) -> Self {
        Self {
            user: user.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        }
    }
}

/// A cached authorization decision with expiration time.
struct CachedDecision {
    allowed: bool,
    expires_at: Instant,
}

/// Thread-safe decision cache with TTL support.
pub struct DecisionCache {
    entries: DashMap<CacheKey, CachedDecision>,
    ttl: Duration,
}

impl DecisionCache {
    /// Create a new decision cache with the given TTL in seconds.
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: DashMap::new(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Get a cached decision if it exists and hasn't expired.
    pub fn get(&self, key: &CacheKey) -> Option<bool> {
        self.entries.get(key).and_then(|entry| {
            if entry.expires_at > Instant::now() {
                Some(entry.allowed)
            } else {
                None
            }
        })
    }

    /// Store a decision in the cache.
    pub fn set(&self, key: CacheKey, allowed: bool) {
        self.entries.insert(
            key,
            CachedDecision {
                allowed,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// Invalidate a specific cache entry.
    pub fn invalidate(&self, key: &CacheKey) {
        self.entries.remove(key);
    }

    /// Invalidate all cache entries for a given object.
    pub fn invalidate_object(&self, object: &str) {
        self.entries.retain(|k, _| k.object != object);
    }

    /// Invalidate all cache entries for a given user.
    pub fn invalidate_user(&self, user: &str) {
        self.entries.retain(|k, _| k.user != user);
    }

    /// Clear all cache entries.
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// Remove expired entries from the cache.
    pub fn evict_expired(&self) {
        let now = Instant::now();
        self.entries.retain(|_, v| v.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_cache_get_set() {
        let cache = DecisionCache::new(60);
        let key = CacheKey::new("user:alice", "viewer", "document:1");

        assert!(cache.get(&key).is_none());

        cache.set(key.clone(), true);
        assert_eq!(cache.get(&key), Some(true));
    }

    #[test]
    fn test_cache_expiration() {
        let cache = DecisionCache::new(1); // 1 second TTL
        let key = CacheKey::new("user:alice", "viewer", "document:1");

        cache.set(key.clone(), true);
        assert_eq!(cache.get(&key), Some(true));

        sleep(Duration::from_secs(2));
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_invalidate_object() {
        let cache = DecisionCache::new(60);
        let key1 = CacheKey::new("user:alice", "viewer", "document:1");
        let key2 = CacheKey::new("user:bob", "editor", "document:1");
        let key3 = CacheKey::new("user:alice", "viewer", "document:2");

        cache.set(key1.clone(), true);
        cache.set(key2.clone(), true);
        cache.set(key3.clone(), true);

        cache.invalidate_object("document:1");

        assert!(cache.get(&key1).is_none());
        assert!(cache.get(&key2).is_none());
        assert_eq!(cache.get(&key3), Some(true));
    }
}
