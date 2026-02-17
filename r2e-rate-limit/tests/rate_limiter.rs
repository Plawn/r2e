use r2e_rate_limit::RateLimiter;
use std::thread::sleep;
use std::time::Duration;

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
