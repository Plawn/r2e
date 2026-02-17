# r2e-rate-limit — Test Development Plan

## Current State

- **4 tests** (inline in `src/lib.rs` — core `RateLimiter` only)
- **Coverage**: ~30% — token bucket logic tested, guards and registry untested
- **Gap**: `RateLimitGuard`, `PreAuthRateLimitGuard`, `RateLimitRegistry`, `InMemoryRateLimiter`, `RateLimit` builder, header parsing

---

## Phase 1: InMemoryRateLimiter Backend

**File**: `src/lib.rs` — extend `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `in_memory_allows_within_limit` | `try_acquire("key", 3, 1)` → first 3 calls return `true` |
| `in_memory_blocks_over_limit` | 4th call → `false` |
| `in_memory_refills_after_window` | Sleep past window → tokens replenished |
| `in_memory_independent_keys` | Different key strings → independent buckets |
| `in_memory_different_configs` | Same key, different `max`/`window` per call → uses per-key config |
| `in_memory_default_impl` | `InMemoryRateLimiter::default()` works same as `::new()` |

---

## Phase 2: RateLimitRegistry

**File**: `src/lib.rs` — extend tests

| Test | Description |
|------|-------------|
| `registry_delegates_to_backend` | `Registry::try_acquire()` calls backend's `try_acquire()` |
| `registry_default_uses_in_memory` | `RateLimitRegistry::default()` uses `InMemoryRateLimiter` |
| `registry_custom_backend` | `RateLimitRegistry::new(custom_backend)` uses provided backend |
| `registry_clone_shares_backend` | Cloned registry shares the same backend (Arc) |

---

## Phase 3: RateLimit Builder

**File**: `src/guard.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `rate_limit_global_creates_pre_auth` | `RateLimit::global(5, 60)` → `PreAuthRateLimitGuard` with Global key |
| `rate_limit_per_ip_creates_pre_auth` | `RateLimit::per_ip(5, 60)` → `PreAuthRateLimitGuard` with Ip key |
| `rate_limit_per_user_creates_guard` | `RateLimit::per_user(5, 60)` → `RateLimitGuard` with User key |
| `builder_max_stored` | Builder stores `max` value correctly |
| `builder_window_stored` | Builder stores `window_secs` value correctly |

---

## Phase 4: Key Generation Logic

**File**: `src/guard.rs` — extend tests

### 4.1 Post-Auth Guard Keys

| Test | Description |
|------|-------------|
| `user_key_format` | User key → `"method_name:user:subject_id"` |
| `user_key_anonymous_fallback` | No identity → `"method_name:user:anonymous"` |
| `global_key_format` | Global key → `"method_name:global"` |

### 4.2 Pre-Auth Guard Keys

| Test | Description |
|------|-------------|
| `pre_auth_global_key` | Global → `"method_name:global"` |
| `pre_auth_ip_key_from_header` | `X-Forwarded-For: 1.2.3.4` → `"method_name:ip:1.2.3.4"` |
| `pre_auth_ip_key_missing_header` | No header → `"method_name:ip:unknown"` |
| `pre_auth_ip_multiple_ips` | `X-Forwarded-For: 1.2.3.4, 5.6.7.8` → uses first IP |
| `pre_auth_ip_empty_header` | Empty header value → `"method_name:ip:unknown"` |
| `pre_auth_ip_malformed_header` | Non-UTF8 header → fallback to `"unknown"` |

---

## Phase 5: Guard HTTP Response

**File**: `src/guard.rs` — extend tests

| Test | Description |
|------|-------------|
| `rate_limit_guard_429_response` | Guard check fails → 429 Too Many Requests |
| `rate_limit_guard_json_body` | Response body contains `{"error": "Rate limit exceeded"}` |
| `pre_auth_guard_429_response` | Pre-auth guard fails → 429 |
| `guard_allows_within_limit` | Within rate limit → `Ok(())` |

---

## Phase 6: Edge Cases

**File**: `src/lib.rs` — extend tests

| Test | Description |
|------|-------------|
| `max_zero_always_blocks` | `RateLimiter::new(0, 1s)` → every call returns `false` |
| `max_one_single_burst` | `RateLimiter::new(1, 1s)` → 1st call true, 2nd false |
| `very_large_window` | `window_secs = 86400` → tokens don't refill within test |
| `concurrent_rate_limiting` | 10 threads hitting same key → exactly `max` succeed |

---

## Phase 7: Integration Tests

**File**: extend `example-app/tests/user_controller_test.rs`

| Test | Description |
|------|-------------|
| `per_ip_rate_limit` | Multiple requests from same IP → 429 after limit |
| `per_user_rate_limit` | Authenticated requests → 429 after per-user limit |
| `per_user_independent_users` | Different users → independent rate limits |
| `rate_limit_reset_after_window` | Wait for window to pass → requests allowed again |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 6 | 1h | None |
| Phase 2 | 4 | 1h | None |
| Phase 3 | 5 | 1h | None |
| Phase 4 | 9 | 2h | HeaderMap construction |
| Phase 5 | 4 | 1.5h | Guard context fixtures |
| Phase 6 | 4 | 1h | None |
| Phase 7 | 4 | 2h | example-app setup |
| **Total** | **36** | **~9.5h** | |
