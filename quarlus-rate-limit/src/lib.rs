pub mod guard;
pub use guard::{RateLimitGuard, RateLimitKeyKind};

use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

/// A token-bucket rate limiter keyed by an arbitrary type.
///
/// Each key gets its own independent bucket. Tokens refill at a constant rate.
#[derive(Clone)]
pub struct RateLimiter<K> {
    buckets: Arc<DashMap<K, TokenBucket>>,
    max_tokens: f64,
    window: Duration,
}

impl<K: Eq + Hash + Clone> RateLimiter<K> {
    /// Create a rate limiter that allows `max` requests per `window`.
    pub fn new(max: u64, window: Duration) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            max_tokens: max as f64,
            window,
        }
    }

    /// Try to consume one token for the given key.
    ///
    /// Returns `true` if the request is allowed, `false` if rate-limited.
    pub fn try_acquire(&self, key: &K) -> bool {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.clone()).or_insert_with(|| TokenBucket {
            tokens: self.max_tokens,
            last_refill: now,
        });

        let bucket = entry.value_mut();

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_refill);
        let refill = (elapsed.as_secs_f64() / self.window.as_secs_f64()) * self.max_tokens;
        bucket.tokens = (bucket.tokens + refill).min(self.max_tokens);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Pluggable rate-limit backend
// ---------------------------------------------------------------------------

/// Trait for pluggable rate-limiting backends.
pub trait RateLimitBackend: Send + Sync + 'static {
    /// Try to acquire a permit. Returns `true` if allowed, `false` if rate-limited.
    fn try_acquire(&self, key: &str, max: u64, window_secs: u64) -> bool;
}

/// Per-key bucket with heterogeneous max/window.
struct ConfiguredBucket {
    tokens: f64,
    max_tokens: f64,
    window: Duration,
    last_refill: Instant,
}

/// In-memory token-bucket backend. Each key gets its own bucket whose
/// max/window are determined by the first call for that key.
#[derive(Clone)]
pub struct InMemoryRateLimiter {
    buckets: Arc<DashMap<String, ConfiguredBucket>>,
}

impl InMemoryRateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
        }
    }
}

impl Default for InMemoryRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitBackend for InMemoryRateLimiter {
    fn try_acquire(&self, key: &str, max: u64, window_secs: u64) -> bool {
        let now = Instant::now();
        let max_tokens = max as f64;
        let window = Duration::from_secs(window_secs);

        let mut entry = self
            .buckets
            .entry(key.to_string())
            .or_insert_with(|| ConfiguredBucket {
                tokens: max_tokens,
                max_tokens,
                window,
                last_refill: now,
            });

        let bucket = entry.value_mut();

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_refill);
        let refill = (elapsed.as_secs_f64() / bucket.window.as_secs_f64()) * bucket.max_tokens;
        bucket.tokens = (bucket.tokens + refill).min(bucket.max_tokens);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Clonable handle to a rate-limit backend.
/// Place in the application state with an `impl FromRef`.
#[derive(Clone)]
pub struct RateLimitRegistry {
    backend: Arc<dyn RateLimitBackend>,
}

impl RateLimitRegistry {
    pub fn new(backend: impl RateLimitBackend) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    pub fn try_acquire(&self, key: &str, max: u64, window_secs: u64) -> bool {
        self.backend.try_acquire(key, max, window_secs)
    }
}

impl Default for RateLimitRegistry {
    fn default() -> Self {
        Self::new(InMemoryRateLimiter::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(1));
        assert!(limiter.try_acquire(&"key"));
        assert!(limiter.try_acquire(&"key"));
        assert!(limiter.try_acquire(&"key"));
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(2, Duration::from_secs(1));
        assert!(limiter.try_acquire(&"key"));
        assert!(limiter.try_acquire(&"key"));
        assert!(!limiter.try_acquire(&"key"));
    }

    #[test]
    fn test_rate_limiter_refills() {
        let limiter = RateLimiter::new(2, Duration::from_millis(100));
        assert!(limiter.try_acquire(&"key"));
        assert!(limiter.try_acquire(&"key"));
        assert!(!limiter.try_acquire(&"key"));
        sleep(Duration::from_millis(110));
        assert!(limiter.try_acquire(&"key"));
    }

    #[test]
    fn test_rate_limiter_independent_keys() {
        let limiter = RateLimiter::new(1, Duration::from_secs(1));
        assert!(limiter.try_acquire(&"a"));
        assert!(!limiter.try_acquire(&"a"));
        assert!(limiter.try_acquire(&"b"));
    }
}
