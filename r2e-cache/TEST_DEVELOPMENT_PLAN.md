# r2e-cache — Test Development Plan

## Current State

- **7 tests** (all inline in `src/lib.rs`)
- **Coverage**: ~44% — basic get/set/remove/clear tested
- **Critical issue**: Global `CACHE_BACKEND` singleton causes test isolation problems
- **Gap**: Expiry verification, evict_expired, concurrent access, singleton behavior

---

## Phase 0: Fix Test Isolation (Prerequisite)

**Problem**: All tests share the `OnceLock<Arc<dyn CacheStore>>` singleton. Tests pollute each other's cache state.

**Solution Options**:
1. Add a `#[cfg(test)] fn reset_cache_backend()` that resets the `OnceLock` (unsafe but pragmatic)
2. Move integration tests to separate test binaries (each gets its own process/singleton)
3. Use a per-test `InMemoryStore` instance directly instead of the global singleton

| Task | Description |
|------|-------------|
| `add_test_reset_function` | `#[cfg(test)]` function to reset global singleton between tests |
| `verify_tests_pass_in_any_order` | Run tests with `--test-threads=1` and verify no order-dependent failures |

---

## Phase 1: TtlCache Missing Coverage

**File**: `src/lib.rs` — extend `#[cfg(test)] mod tests`

### 1.1 Expiration Behavior

| Test | Description |
|------|-------------|
| `cache_get_removes_expired_entry` | Get expired key → returns `None` AND entry removed from DashMap |
| `cache_expired_entry_cleaned_on_access` | After expiry, `get()` removes the entry (verify via subsequent insert/size check) |

### 1.2 `evict_expired()` Method

| Test | Description |
|------|-------------|
| `evict_expired_removes_stale` | Insert 3 items with short TTL, sleep, call `evict_expired()` → all removed |
| `evict_expired_keeps_fresh` | Insert items with long TTL, call `evict_expired()` → items preserved |
| `evict_expired_mixed` | Mix of expired and fresh → only expired removed |
| `evict_expired_empty_cache` | `evict_expired()` on empty cache → no panic |

### 1.3 Generic Type Support

| Test | Description |
|------|-------------|
| `cache_with_integer_keys` | `TtlCache<i64, String>` works correctly |
| `cache_with_struct_values` | `TtlCache<String, MyStruct>` where `MyStruct: Clone` |

---

## Phase 2: InMemoryStore Missing Coverage

**File**: `src/lib.rs` — extend tests

### 2.1 TTL Enforcement

| Test | Description |
|------|-------------|
| `in_memory_store_ttl_expiration` | Set with short TTL, sleep, get → `None` |
| `in_memory_store_lazy_eviction` | Expired entry removed from DashMap on get (verify via internal state) |

### 2.2 Clear

| Test | Description |
|------|-------------|
| `in_memory_store_clear` | Set multiple keys, clear → all gone |
| `in_memory_store_clear_empty` | Clear on empty store → no panic |

### 2.3 Remove

| Test | Description |
|------|-------------|
| `in_memory_store_remove_existing` | Remove existing key → no longer retrievable |
| `in_memory_store_remove_nonexistent` | Remove non-existent key → no panic |

---

## Phase 3: Concurrent Access

**File**: `tests/concurrency_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `concurrent_reads_writes` | 10 threads doing get/set simultaneously → no panic, data consistent |
| `concurrent_eviction` | Threads calling `evict_expired()` while others read/write → no panic |
| `clone_shares_state` | `TtlCache::clone()` → both clones see same data |
| `cache_store_concurrent_access` | Multiple tokio tasks using same `InMemoryStore` → consistent |

---

## Phase 4: Global Singleton Behavior

**File**: `src/lib.rs` — extend tests (or in separate test binary)

| Test | Description |
|------|-------------|
| `set_cache_backend_once` | First call to `set_cache_backend()` succeeds |
| `set_cache_backend_twice_ignored` | Second call silently ignored (no panic) |
| `cache_backend_default_lazy_init` | `cache_backend()` without prior `set_cache_backend()` → returns `InMemoryStore` |
| `cache_backend_returns_custom` | After `set_cache_backend(custom)` → `cache_backend()` returns custom |

---

## Phase 5: CacheStore Trait Compliance

**File**: `tests/custom_store_test.rs` (new)

Verify the trait contract with a mock implementation.

| Test | Description |
|------|-------------|
| `custom_store_get_set` | Custom `CacheStore` impl → get/set round-trip |
| `custom_store_remove` | Custom store → remove works |
| `custom_store_clear` | Custom store → clear works |
| `custom_store_remove_by_prefix` | Custom store → prefix removal works |
| `custom_store_send_sync` | Custom store is `Send + Sync + 'static` (compile check) |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 0 | 2 | 1h | None |
| Phase 1 | 8 | 1.5h | None |
| Phase 2 | 6 | 1h | None |
| Phase 3 | 4 | 1.5h | tokio multi-thread |
| Phase 4 | 4 | 1h | Separate test binary |
| Phase 5 | 5 | 1h | None |
| **Total** | **29** | **~7h** | |
