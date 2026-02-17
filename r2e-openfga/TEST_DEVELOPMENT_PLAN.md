# r2e-openfga — Test Development Plan

## Current State

- **14 tests** (cache: 3, registry: 4, backend: 3, guard: 4+)
- **Coverage**: ~70% — best-tested crate in the workspace
- **Gap**: OpenFgaConfig, GrpcBackend, cache capacity eviction, concurrent access, guard integration

---

## Phase 1: OpenFgaConfig (Quick Win)

**File**: `src/config.rs` — add `#[cfg(test)] mod tests` (if not present)

| Test | Description |
|------|-------------|
| `config_new_required_fields` | `OpenFgaConfig::new(endpoint, store_id)` stores fields |
| `config_with_model_id` | `.with_model_id("model")` sets model_id |
| `config_with_api_token` | `.with_api_token("token")` sets token |
| `config_with_cache` | `.with_cache(config)` enables caching |
| `config_default_timeout` | Default timeout value is reasonable |

---

## Phase 2: DecisionCache Edge Cases

**File**: `src/cache.rs` — extend tests

| Test | Description |
|------|-------------|
| `cache_capacity_eviction` | Insert beyond `max_entries` → oldest evicted |
| `cache_evict_expired_cleans` | `evict_expired()` removes stale entries |
| `cache_evict_expired_keeps_fresh` | Fresh entries survive eviction |
| `invalidate_user_removes_all_user_tuples` | `invalidate_user("user:alice")` removes all entries with that user |
| `clear_removes_everything` | `clear()` → empty cache |
| `get_expired_returns_none` | Expired entry → `None` (not stale data) |

---

## Phase 3: MockBackend Extended

**File**: `src/backend.rs` — extend tests

| Test | Description |
|------|-------------|
| `mock_check_missing_tuple` | Check non-existent tuple → `false` |
| `mock_list_objects_empty` | No matching tuples → empty vec |
| `mock_list_objects_filters_by_relation` | Only tuples with matching relation returned |
| `mock_has_tuple` | `has_tuple()` returns `true` for existing tuples |
| `mock_remove_nonexistent` | Remove non-existent tuple → no panic |

---

## Phase 4: Registry Without Cache

**File**: `src/registry.rs` — extend tests

| Test | Description |
|------|-------------|
| `registry_no_cache_always_checks_backend` | Without cache → every check hits backend |
| `registry_invalidate_without_cache_no_panic` | Invalidation methods don't panic when no cache |
| `registry_clear_without_cache_no_panic` | Clear doesn't panic without cache |

---

## Phase 5: Guard Integration (Full Check Flow)

**File**: `tests/guard_integration_test.rs` (new)

| Test | Description |
|------|-------------|
| `fga_guard_allows_authorized` | User has tuple → guard returns `Ok(())` |
| `fga_guard_denies_unauthorized` | User lacks tuple → guard returns 403 |
| `fga_guard_extracts_path_param` | Object resolved from `Path("123")` → check uses `"type:123"` |
| `fga_guard_extracts_query_param` | Object from query string → correct check |
| `fga_guard_extracts_header` | Object from header → correct check |
| `fga_guard_uses_identity_sub` | User subject from `Identity::sub()` → correct user in check |

---

## Phase 6: GrpcBackend (Optional, Requires Infrastructure)

These tests require a running OpenFGA server or extensive mocking.

| Test | Description |
|------|-------------|
| `grpc_connect_success` | Connection to local OpenFGA → `GrpcBackend` created |
| `grpc_connect_timeout` | Invalid endpoint → timeout error |
| `grpc_check_authorized` | Write tuple + check → `true` |
| `grpc_check_unauthorized` | No tuple → `false` |
| `grpc_list_objects` | Written tuples → listed correctly |
| `grpc_api_token_header` | Bearer token sent in gRPC metadata |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 5 | 30m | None |
| Phase 2 | 6 | 1.5h | None |
| Phase 3 | 5 | 1h | None |
| Phase 4 | 3 | 30m | None |
| Phase 5 | 6 | 3h | Guard context fixtures |
| Phase 6 | 6 | 4h | OpenFGA server or tonic mock |
| **Total** | **31** | **~10.5h** | |
