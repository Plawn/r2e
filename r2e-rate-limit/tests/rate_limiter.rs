use r2e_rate_limit::{InMemoryRateLimiter, RateLimitBackend, RateLimiter};
use std::sync::{Arc, Barrier};
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

#[test]
#[should_panic(expected = "`window` must be greater than zero")]
fn test_rate_limiter_rejects_zero_window() {
    let _ = RateLimiter::<&str>::new(5, Duration::ZERO);
}

// ── InMemoryRateLimiter: budgets are re-tuned in place ──────────────────────

#[test]
fn in_memory_backend_tightens_an_existing_bucket() {
    let backend = InMemoryRateLimiter::new();
    // Bucket created with a budget of 5, one token spent (4 left).
    assert!(backend.try_acquire("k", 5, 3600));
    // Budget drops to 1: the stored tokens are clamped to the new maximum, so
    // exactly one more call gets through instead of the four the old budget
    // still held.
    assert!(backend.try_acquire("k", 1, 3600));
    assert!(!backend.try_acquire("k", 1, 3600));
    assert!(!backend.try_acquire("k", 1, 3600));
}

#[test]
fn in_memory_backend_widens_an_existing_bucket() {
    let backend = InMemoryRateLimiter::new();
    // Exhaust a bucket created with a budget of 1 per second.
    assert!(backend.try_acquire("k", 1, 1));
    assert!(!backend.try_acquire("k", 1, 1));

    // Raise the budget to 10/s. After 200 ms the new rate has accrued ~2
    // tokens; the creation-time budget would have accrued only ~0.2 — not
    // enough for a single call.
    sleep(Duration::from_millis(200));
    assert!(backend.try_acquire("k", 10, 1));
    assert!(backend.try_acquire("k", 10, 1));
}

#[test]
fn in_memory_backend_retunes_the_window() {
    let backend = InMemoryRateLimiter::new();
    // Exhaust a bucket created with a long window.
    assert!(backend.try_acquire("k", 1, 3600));
    assert!(!backend.try_acquire("k", 1, 3600));
    // The window shrinks to 1s: after a second the bucket refills, which would
    // not happen if the bucket kept its original hour-long window.
    sleep(Duration::from_millis(1100));
    assert!(backend.try_acquire("k", 1, 1));
}

#[test]
fn in_memory_backend_never_refills_a_zero_window() {
    // No public constructor mints a zero window; if one ever reaches the
    // backend the bucket must fail closed rather than become unlimited.
    let backend = InMemoryRateLimiter::new();
    assert!(backend.try_acquire("k", 1, 0));
    for _ in 0..5 {
        assert!(!backend.try_acquire("k", 1, 0));
    }
}

#[test]
fn in_memory_backend_survives_racing_callers_with_different_budgets() {
    // Two budgets hammering the same key concurrently: the bucket must stay
    // coherent (never hand out more than the widest budget's worth of tokens,
    // never panic, never deadlock).
    let backend = Arc::new(InMemoryRateLimiter::new());
    let threads = 8;
    let per_thread = 200;
    let barrier = Arc::new(Barrier::new(threads));

    let handles: Vec<_> = (0..threads)
        .map(|i| {
            let backend = Arc::clone(&backend);
            let barrier = Arc::clone(&barrier);
            // Half the callers present a tight budget, half a wide one.
            let max = if i % 2 == 0 { 1 } else { 50 };
            std::thread::spawn(move || {
                barrier.wait();
                let mut granted = 0usize;
                for _ in 0..per_thread {
                    if backend.try_acquire("shared", max, 3600) {
                        granted += 1;
                    }
                }
                granted
            })
        })
        .collect();

    let granted: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    // With a one-hour window nothing meaningfully refills during the test, so
    // the total handed out is bounded by the widest budget seen.
    assert!(
        granted >= 1 && granted <= 50,
        "expected 1..=50 grants under racing budgets, got {granted}"
    );
}
