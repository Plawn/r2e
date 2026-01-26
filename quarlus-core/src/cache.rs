use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A thread-safe TTL cache backed by `DashMap`.
///
/// Entries expire after the configured `ttl` and are lazily evicted on access.
#[derive(Clone)]
pub struct TtlCache<K, V> {
    inner: Arc<DashMap<K, (V, Instant)>>,
    ttl: Duration,
}

impl<K: Eq + Hash + Clone, V: Clone> TtlCache<K, V> {
    /// Create a new cache with the given time-to-live.
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Get a cached value if it exists and hasn't expired.
    pub fn get(&self, key: &K) -> Option<V> {
        if let Some(entry) = self.inner.get(key) {
            let (val, inserted) = entry.value();
            if inserted.elapsed() < self.ttl {
                return Some(val.clone());
            }
            // Expired — drop the read guard before removing
            drop(entry);
            self.inner.remove(key);
        }
        None
    }

    /// Insert or update a value in the cache.
    pub fn insert(&self, key: K, value: V) {
        self.inner.insert(key, (value, Instant::now()));
    }

    /// Remove all expired entries.
    pub fn evict_expired(&self) {
        self.inner.retain(|_, (_, inserted)| inserted.elapsed() < self.ttl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_cache_hit() {
        let cache = TtlCache::new(Duration::from_secs(60));
        cache.insert("key", "value");
        assert_eq!(cache.get(&"key"), Some("value"));
    }

    #[test]
    fn test_cache_miss() {
        let cache: TtlCache<&str, &str> = TtlCache::new(Duration::from_secs(60));
        assert_eq!(cache.get(&"missing"), None);
    }

    #[test]
    fn test_cache_expiry() {
        let cache = TtlCache::new(Duration::from_millis(50));
        cache.insert("key", "value");
        assert_eq!(cache.get(&"key"), Some("value"));
        sleep(Duration::from_millis(60));
        assert_eq!(cache.get(&"key"), None);
    }
}
