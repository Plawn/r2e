use r2e_cache::{InMemoryStore, CacheStore, TtlCache};
use bytes::Bytes;
use std::thread::sleep;
use std::time::Duration;

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

#[tokio::test]
async fn test_in_memory_store() {
    let store = InMemoryStore::new();
    store.set("k1", Bytes::from("v1"), Duration::from_secs(60)).await;
    assert_eq!(store.get("k1").await, Some(Bytes::from("v1")));

    store.remove("k1").await;
    assert_eq!(store.get("k1").await, None);
}

#[tokio::test]
async fn test_in_memory_store_prefix_removal() {
    let store = InMemoryStore::new();
    store.set("users:1", Bytes::from("a"), Duration::from_secs(60)).await;
    store.set("users:2", Bytes::from("b"), Duration::from_secs(60)).await;
    store.set("posts:1", Bytes::from("c"), Duration::from_secs(60)).await;

    store.remove_by_prefix("users:").await;
    assert_eq!(store.get("users:1").await, None);
    assert_eq!(store.get("users:2").await, None);
    assert_eq!(store.get("posts:1").await, Some(Bytes::from("c")));
}
