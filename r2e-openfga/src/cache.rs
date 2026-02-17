//! Decision cache for OpenFGA authorization checks.

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Default maximum number of entries in the cache.
const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Interval between automatic eviction sweeps (triggered lazily on `set()`).
const EVICTION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

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

/// Thread-safe decision cache with TTL and maximum capacity.
///
/// Expired entries are evicted lazily: a background sweep runs at most once per
/// minute, triggered by `set()` calls. This keeps the memory footprint bounded
/// without requiring a dedicated eviction task.
///
/// When the cache reaches `max_entries`, all expired entries are evicted first.
/// If the cache is still full after eviction, the new entry is **not** inserted
/// (fail-open: the backend is queried every time).
pub struct DecisionCache {
    entries: DashMap<CacheKey, CachedDecision>,
    ttl: Duration,
    max_entries: usize,
    len: AtomicUsize,
    last_eviction: std::sync::Mutex<Instant>,
}

impl DecisionCache {
    /// Create a new decision cache with the given TTL in seconds.
    pub fn new(ttl_secs: u64) -> Self {
        Self::with_capacity(ttl_secs, DEFAULT_MAX_ENTRIES)
    }

    /// Create a new decision cache with the given TTL and maximum entry count.
    pub fn with_capacity(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            entries: DashMap::new(),
            ttl: Duration::from_secs(ttl_secs),
            max_entries,
            len: AtomicUsize::new(0),
            last_eviction: std::sync::Mutex::new(Instant::now()),
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
    ///
    /// If the cache is at capacity, expired entries are evicted first.
    /// If still full after eviction, the entry is not inserted.
    pub fn set(&self, key: CacheKey, allowed: bool) {
        self.maybe_evict();

        if self.len.load(Ordering::Relaxed) >= self.max_entries {
            // Force an eviction pass and recheck.
            self.evict_expired();
            if self.len.load(Ordering::Relaxed) >= self.max_entries {
                return;
            }
        }

        let was_absent = self
            .entries
            .insert(
                key,
                CachedDecision {
                    allowed,
                    expires_at: Instant::now() + self.ttl,
                },
            )
            .is_none();
        if was_absent {
            self.len.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Invalidate a specific cache entry.
    pub fn invalidate(&self, key: &CacheKey) {
        if self.entries.remove(key).is_some() {
            self.len.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Invalidate all cache entries for a given object.
    pub fn invalidate_object(&self, object: &str) {
        self.entries.retain(|k, _| {
            let keep = k.object != object;
            if !keep {
                self.len.fetch_sub(1, Ordering::Relaxed);
            }
            keep
        });
    }

    /// Invalidate all cache entries for a given user.
    pub fn invalidate_user(&self, user: &str) {
        self.entries.retain(|k, _| {
            let keep = k.user != user;
            if !keep {
                self.len.fetch_sub(1, Ordering::Relaxed);
            }
            keep
        });
    }

    /// Clear all cache entries.
    pub fn clear(&self) {
        self.entries.clear();
        self.len.store(0, Ordering::Relaxed);
    }

    /// Remove expired entries from the cache.
    pub fn evict_expired(&self) {
        let now = Instant::now();
        self.entries.retain(|_, v| {
            let keep = v.expires_at > now;
            if !keep {
                self.len.fetch_sub(1, Ordering::Relaxed);
            }
            keep
        });
        if let Ok(mut last) = self.last_eviction.lock() {
            *last = Instant::now();
        }
    }

    /// Current number of entries (including potentially expired ones).
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Evict expired entries if enough time has passed since the last sweep.
    fn maybe_evict(&self) {
        let should_evict = self
            .last_eviction
            .lock()
            .map(|last| last.elapsed() >= EVICTION_CHECK_INTERVAL)
            .unwrap_or(false);
        if should_evict {
            self.evict_expired();
        }
    }
}
