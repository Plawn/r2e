# r2e-utils — Test Development Plan

## Current State

- **8 tests** (inline in `src/interceptors.rs`)
- **Coverage**: ~60% — Logged, Timed, Cache, CacheInvalidate tested; Counted, MetricTimed not tested
- **Gap**: New interceptors, edge cases, concurrent access

---

## Phase 1: Counted Interceptor

**File**: `src/interceptors.rs` — extend `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `counted_new_default_level` | `Counted::new("counter")` uses default log level |
| `counted_with_level` | `Counted::new("counter").with_level(LogLevel::Debug)` stores level |
| `counted_around_executes_inner` | `around()` calls inner function and returns result |
| `counted_increments` | After `around()`, counter value incremented (verify via logs or state) |

---

## Phase 2: MetricTimed Interceptor

| Test | Description |
|------|-------------|
| `metric_timed_new` | `MetricTimed::new("timer")` creates with name |
| `metric_timed_with_level` | `.with_level(LogLevel::Info)` stores level |
| `metric_timed_around_measures_duration` | `around()` records elapsed time |
| `metric_timed_around_executes_inner` | Inner function called and result returned |

---

## Phase 3: Edge Cases for Existing Interceptors

### 3.1 Timed Threshold Boundary

| Test | Description |
|------|-------------|
| `timed_threshold_exact_boundary` | Elapsed == threshold → logged (inclusive) |
| `timed_threshold_below` | Elapsed < threshold → not logged |
| `timed_threshold_above` | Elapsed > threshold → logged |

### 3.2 Cache Edge Cases

| Test | Description |
|------|-------------|
| `cache_with_custom_key` | `Cache::ttl(30).with_key("custom")` → uses "custom" in cache key |
| `cache_ttl_expiration` | Cached value expires after TTL → fresh call on next request |
| `cache_deserialization_failure` | Corrupted cache entry → cache miss, entry removed |
| `cache_group_isolation` | Different groups → independent cache spaces |

### 3.3 CacheInvalidate Edge Cases

| Test | Description |
|------|-------------|
| `cache_invalidate_nonexistent_group` | Invalidating non-existent group → no panic |
| `cache_invalidate_after_method` | Invalidation happens after successful method execution |

---

## Phase 4: Interceptor Composition

| Test | Description |
|------|-------------|
| `three_interceptors_nested` | Logged → Timed → Cache → all execute in order |
| `interceptor_error_propagation` | Inner function returns error → interceptors still complete |
| `interceptor_with_different_return_types` | Works with `Json<T>`, `String`, `StatusCode` return types |

---

## Phase 5: log_at_level Utility

| Test | Description |
|------|-------------|
| `log_at_level_info` | `log_at_level(LogLevel::Info, ...)` → calls tracing::info |
| `log_at_level_debug` | `log_at_level(LogLevel::Debug, ...)` → calls tracing::debug |
| `log_at_level_warn` | `log_at_level(LogLevel::Warn, ...)` → calls tracing::warn |
| `log_at_level_error` | `log_at_level(LogLevel::Error, ...)` → calls tracing::error |
| `log_at_level_trace` | `log_at_level(LogLevel::Trace, ...)` → calls tracing::trace |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 4 | 1h | None |
| Phase 2 | 4 | 1h | None |
| Phase 3 | 8 | 2h | Cache/timer fixtures |
| Phase 4 | 3 | 1h | None |
| Phase 5 | 5 | 1h | tracing-test |
| **Total** | **24** | **~6h** | |
