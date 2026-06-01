# r2e-data-sqlx — Test Development Plan

## Current State

- **0 tests**
- **Coverage**: 0%
- **Gap**: SqlxRepository, Tx lifecycle, ManagedResource, error mapping, HasPool — all untested

---

## Phase 1: Error Mapping (Pure Logic)

**File**: `src/lib.rs` or `src/error.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `sqlx_row_not_found_maps_to_not_found` | `sqlx::Error::RowNotFound` → `DataError::NotFound` |
| `sqlx_other_error_maps_to_database` | Other sqlx errors → `DataError::Database` |
| `sqlx_connection_error_maps_to_database` | Connection failure → `DataError::Database` |
| `error_message_preserved` | Original error message accessible via `source()` |

---

## Phase 2: Tx Lifecycle (In-Memory SQLite)

**File**: `tests/tx_test.rs` (new integration test)

Requires: `sqlx = { features = ["sqlite", "runtime-tokio"] }` as dev-dependency.

| Test | Description |
|------|-------------|
| `tx_acquire_success` | `Tx::acquire(&state)` → returns valid transaction |
| `tx_acquire_pool_error` | Pool closed → `Err` with meaningful error |
| `tx_release_true_commits` | `release(true)` → data persisted in database |
| `tx_release_false_rollbacks` | `release(false)` → data NOT persisted |
| `tx_drop_without_release_rollbacks` | Drop Tx without calling release → implicit rollback |
| `tx_deref_provides_connection` | `&*tx` gives access to underlying transaction |
| `tx_deref_mut_allows_queries` | `tx.as_mut()` usable with `sqlx::query().execute()` |
| `tx_into_inner_unwraps` | `tx.into_inner()` returns the raw `Transaction` |
| `tx_multiple_operations` | Multiple queries within same tx → all committed together |
| `tx_error_mid_operation` | Query fails mid-tx → release(false) rolls back prior operations |

---

## Phase 3: SqlxRepository

**File**: `tests/repository_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `repository_new` | `SqlxRepository::new(pool)` creates instance |
| `repository_pool_accessor` | `.pool()` returns reference to the pool |
| `repository_clone` | Cloned repository shares the same pool |

---

## Phase 4: HasPool Trait

**File**: `tests/has_pool_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `has_pool_extraction` | `HasPool<Sqlite>::pool(&state)` returns pool reference |
| `has_pool_from_ref` | Pool extractable from state via `FromRef` |

---

## Phase 5: Full CRUD Integration (Optional, High Value)

**File**: `tests/crud_integration_test.rs` (new)

Uses in-memory SQLite with migration.

| Test | Description |
|------|-------------|
| `find_by_id_existing` | Seeded record → `Some(entity)` |
| `find_by_id_missing` | Non-existent ID → `None` |
| `find_all` | Multiple records → all returned |
| `find_all_paged` | 25 records, page=1, size=10 → 10 results, total_pages=3 |
| `save_insert` | New entity → inserted, ID assigned |
| `save_update` | Existing entity → updated in place |
| `delete_existing` | Existing ID → deleted, returns `true` |
| `delete_missing` | Non-existent ID → returns `false` |
| `count_empty` | Empty table → 0 |
| `count_with_records` | 5 records → 5 |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 4 | 30m | None |
| Phase 2 | 10 | 3h | SQLite in-memory |
| Phase 3 | 3 | 30m | SQLite pool |
| Phase 4 | 2 | 30m | State fixture |
| Phase 5 | 10 | 4h | Migration + entity fixture |
| **Total** | **29** | **~8.5h** | |

---

## Notes

- All integration tests should use `sqlx::SqlitePool::connect("sqlite::memory:")` for isolation.
- Each test should create its own schema to avoid cross-test contamination.
- Consider using `#[sqlx::test]` macro if available for automatic pool/migration setup.
