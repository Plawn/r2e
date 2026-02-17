use r2e_openfga::cache::{CacheKey, DecisionCache};
use std::thread::sleep;
use std::time::Duration;

#[test]
fn test_cache_get_set() {
    let cache = DecisionCache::new(60);
    let key = CacheKey::new("user:alice", "viewer", "document:1");

    assert!(cache.get(&key).is_none());

    cache.set(key.clone(), true);
    assert_eq!(cache.get(&key), Some(true));
    assert_eq!(cache.len(), 1);
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
    assert_eq!(cache.len(), 1);
}

#[test]
fn test_cache_max_capacity() {
    let cache = DecisionCache::with_capacity(60, 3);

    cache.set(CacheKey::new("user:a", "viewer", "doc:1"), true);
    cache.set(CacheKey::new("user:b", "viewer", "doc:2"), true);
    cache.set(CacheKey::new("user:c", "viewer", "doc:3"), true);
    assert_eq!(cache.len(), 3);

    // This should be silently dropped (cache full, no expired entries).
    cache.set(CacheKey::new("user:d", "viewer", "doc:4"), true);
    assert_eq!(cache.len(), 3);
    assert!(cache.get(&CacheKey::new("user:d", "viewer", "doc:4")).is_none());
}

#[test]
fn test_cache_evicts_expired_before_rejecting() {
    let cache = DecisionCache::with_capacity(1, 2); // 1s TTL, max 2

    cache.set(CacheKey::new("user:a", "viewer", "doc:1"), true);
    cache.set(CacheKey::new("user:b", "viewer", "doc:2"), true);
    assert_eq!(cache.len(), 2);

    sleep(Duration::from_secs(2)); // both entries expire

    // Should succeed: expired entries are evicted first.
    cache.set(CacheKey::new("user:c", "viewer", "doc:3"), true);
    assert_eq!(
        cache.get(&CacheKey::new("user:c", "viewer", "doc:3")),
        Some(true)
    );
}

#[test]
fn test_cache_clear() {
    let cache = DecisionCache::new(60);
    cache.set(CacheKey::new("user:a", "viewer", "doc:1"), true);
    cache.set(CacheKey::new("user:b", "viewer", "doc:2"), true);
    assert_eq!(cache.len(), 2);

    cache.clear();
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
}
