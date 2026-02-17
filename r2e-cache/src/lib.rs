use bytes::Bytes;
use dashmap::DashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
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

// ---------------------------------------------------------------------------
// CacheStore trait + InMemoryStore + global singleton
// ---------------------------------------------------------------------------

/// Pluggable cache backend trait.
///
/// Implement this to swap the default in-memory store for Redis, Memcached, etc.
/// Register your implementation at startup via [`set_cache_backend`].
pub trait CacheStore: Send + Sync + 'static {
    fn get<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = Option<Bytes>> + Send + 'a>>;
    fn set<'a>(&'a self, key: &'a str, value: Bytes, ttl: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    fn remove<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    fn clear(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    fn remove_by_prefix<'a>(&'a self, prefix: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Default in-memory cache store backed by `DashMap`.
///
/// Each entry stores `(value, inserted_at, ttl)` and is lazily evicted on access.
#[derive(Clone)]
pub struct InMemoryStore {
    inner: Arc<DashMap<String, (Bytes, Instant, Duration)>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheStore for InMemoryStore {
    fn get<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = Option<Bytes>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(entry) = self.inner.get(key) {
                let (val, inserted, ttl) = entry.value();
                if inserted.elapsed() < *ttl {
                    return Some(val.clone());
                }
                drop(entry);
                self.inner.remove(key);
            }
            None
        })
    }

    fn set<'a>(&'a self, key: &'a str, value: Bytes, ttl: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.inner.insert(key.to_string(), (value, Instant::now(), ttl));
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.inner.remove(key);
        })
    }

    fn clear(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.inner.clear();
        })
    }

    fn remove_by_prefix<'a>(&'a self, prefix: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.inner.retain(|k, _| !k.starts_with(prefix));
        })
    }
}

// Global singleton
use std::sync::OnceLock;

static CACHE_BACKEND: OnceLock<Arc<dyn CacheStore>> = OnceLock::new();

/// Set a custom cache backend. Must be called before any cache operations.
/// Typically called once at application startup.
pub fn set_cache_backend(store: impl CacheStore) {
    let _ = CACHE_BACKEND.set(Arc::new(store));
}

/// Get the current cache backend. Defaults to [`InMemoryStore`] if none was set.
pub fn cache_backend() -> Arc<dyn CacheStore> {
    CACHE_BACKEND
        .get()
        .cloned()
        .unwrap_or_else(|| {
            // Lazy-init with InMemoryStore on first access
            let store: Arc<dyn CacheStore> = Arc::new(InMemoryStore::new());
            // Try to set it; if another thread beat us, use theirs
            let _ = CACHE_BACKEND.set(store.clone());
            CACHE_BACKEND.get().cloned().unwrap_or(store)
        })
}
