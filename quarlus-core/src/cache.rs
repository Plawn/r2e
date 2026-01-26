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

    /// Remove a specific entry from the cache.
    pub fn remove(&self, key: &K) {
        self.inner.remove(key);
    }

    /// Remove all entries from the cache.
    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Remove all expired entries.
    pub fn evict_expired(&self) {
        self.inner.retain(|_, (_, inserted)| inserted.elapsed() < self.ttl);
    }
}

/// Global registry of named cache groups.
///
/// Used by `#[cached(group = "...")]` and `#[cache_invalidate("...")]`
/// to share caches across methods and invalidate them by name.
pub struct CacheRegistry;

use std::sync::OnceLock;

static CACHE_REGISTRY: OnceLock<DashMap<String, TtlCache<String, String>>> = OnceLock::new();

impl CacheRegistry {
    fn registry() -> &'static DashMap<String, TtlCache<String, String>> {
        CACHE_REGISTRY.get_or_init(DashMap::new)
    }

    /// Get or create a named cache group with the given TTL.
    pub fn get_or_create(group: &str, ttl: Duration) -> TtlCache<String, String> {
        let reg = Self::registry();
        reg.entry(group.to_string())
            .or_insert_with(|| TtlCache::new(ttl))
            .clone()
    }

    /// Invalidate (clear) all entries in a named cache group.
    pub fn invalidate(group: &str) {
        let reg = Self::registry();
        if let Some(cache) = reg.get(group) {
            cache.clear();
        }
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

    #[test]
    fn test_cache_remove() {
        let cache = TtlCache::new(Duration::from_secs(60));
        cache.insert("key", "value");
        assert_eq!(cache.get(&"key"), Some("value"));
        cache.remove(&"key");
        assert_eq!(cache.get(&"key"), None);
    }

    #[test]
    fn test_cache_clear() {
        let cache = TtlCache::new(Duration::from_secs(60));
        cache.insert("a", "1");
        cache.insert("b", "2");
        cache.clear();
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), None);
    }

    #[test]
    fn test_cache_registry() {
        let cache = CacheRegistry::get_or_create("test_group", Duration::from_secs(60));
        cache.insert("key".to_string(), "value".to_string());

        let same_cache = CacheRegistry::get_or_create("test_group", Duration::from_secs(60));
        assert_eq!(same_cache.get(&"key".to_string()), Some("value".to_string()));

        CacheRegistry::invalidate("test_group");
        assert_eq!(cache.get(&"key".to_string()), None);
    }
}
