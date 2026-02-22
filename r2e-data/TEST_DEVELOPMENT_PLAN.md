# r2e-data — Test Development Plan

## Current State

- **0 tests** across r2e-data, r2e-data-sqlx, and r2e-data-diesel
- **Coverage**: 0%
- **Gap**: All public APIs untested — Entity, Repository, Page, Pageable, DataError, QueryBuilder

---

## Phase 1: Pageable & Page (Pure Logic, Quick Win)

**File**: `src/lib.rs` or `src/page.rs` — add `#[cfg(test)] mod tests`

### 1.1 Pageable

| Test | Description |
|------|-------------|
| `pageable_default` | `Pageable::default()` → page=0, size=20, sort=None |
| `pageable_offset_page_0` | page=0, size=10 → offset=0 |
| `pageable_offset_page_1` | page=1, size=10 → offset=10 |
| `pageable_offset_page_5` | page=5, size=20 → offset=100 |
| `pageable_offset_size_1` | page=3, size=1 → offset=3 |
| `pageable_custom_sort` | sort=Some("name") stored correctly |
| `pageable_deserialization` | Query string `?page=2&size=10&sort=name` → correct struct |

### 1.2 Page

| Test | Description |
|------|-------------|
| `page_total_pages_exact` | 20 items, size=10 → total_pages=2 |
| `page_total_pages_remainder` | 25 items, size=10 → total_pages=3 |
| `page_total_pages_single` | 5 items, size=10 → total_pages=1 |
| `page_total_pages_zero_items` | 0 items, size=10 → total_pages=0 |
| `page_total_pages_one_item` | 1 item, size=10 → total_pages=1 |
| `page_total_pages_size_1` | 5 items, size=1 → total_pages=5 |
| `page_size_zero_no_panic` | size=0 → total_pages=0 (or handled gracefully) |
| `page_content_preserved` | Content vec stored as-is |
| `page_metadata_correct` | page, size, total_elements fields match input |
| `page_serialization` | `Page<User>` serializes to expected JSON structure |

---

## Phase 2: DataError

**File**: `src/error.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `data_error_not_found_display` | `DataError::NotFound("User 123")` → `"Not found: User 123"` |
| `data_error_database_display` | `DataError::Database(err)` → contains error message |
| `data_error_other_display` | `DataError::Other(msg)` → expected message |
| `data_error_database_source` | `.source()` returns the wrapped error |
| `data_error_not_found_source` | `.source()` returns `None` |
| `data_error_to_app_error_not_found` | `DataError::NotFound` → `HttpError::NotFound` |
| `data_error_to_app_error_database` | `DataError::Database` → `HttpError::Internal` |
| `data_error_send_sync` | `DataError` is `Send + Sync` (compile-time check) |

---

## Phase 3: Entity & QueryBuilder

**File**: add `#[cfg(test)] mod tests` in relevant source file

### 3.1 Entity Trait (Compile-Time Verification)

| Test | Description |
|------|-------------|
| `entity_table_name` | `User::table_name()` returns `"users"` |
| `entity_id_column` | `User::id_column()` returns `"id"` |
| `entity_columns` | `User::columns()` returns expected column list |
| `entity_id_accessor` | `user.id()` returns id value |

### 3.2 QueryBuilder (if present)

| Test | Description |
|------|-------------|
| `query_builder_where_eq` | `.where_eq("name", "Alice")` generates correct clause |
| `query_builder_where_like` | `.where_like("name", "%Ali%")` generates LIKE clause |
| `query_builder_order_by` | `.order_by("name", Asc)` appends ORDER BY |
| `query_builder_limit` | `.limit(10)` appends LIMIT |
| `query_builder_offset` | `.offset(20)` appends OFFSET |
| `query_builder_combined` | Multiple clauses chain correctly |
| `query_builder_empty` | No clauses → valid base query |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 17 | 2h | None |
| Phase 2 | 8 | 1h | r2e-core (for HttpError) |
| Phase 3 | 11 | 2h | Test entity fixture |
| **Total** | **36** | **~5h** | |
