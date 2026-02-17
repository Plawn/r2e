# r2e-scheduler — Test Development Plan

## Current State

- **0 runtime tests** (compile tests in r2e-compile-tests only)
- **Coverage**: 0% runtime — syntax validation only
- **Gap**: Interval execution, cron scheduling, cancellation, state capture, error handling, SchedulerHandle extraction — all untested at runtime

---

## Phase 1: ScheduleConfig & ScheduledResult (Unit Tests)

**File**: `src/types.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `interval_config_creation` | `ScheduleConfig::Interval(Duration::from_secs(30))` stores correctly |
| `interval_with_delay_creation` | `ScheduleConfig::IntervalWithDelay { interval, initial_delay }` stores both |
| `cron_config_creation` | `ScheduleConfig::Cron("0 */5 * * * *".into())` stores expression |
| `scheduled_result_unit` | `().log_if_err("task")` → no-op, no panic |
| `scheduled_result_ok` | `Ok::<(), String>(()).log_if_err("task")` → no-op |
| `scheduled_result_err` | `Err::<(), _>("fail").log_if_err("task")` → logs error, no panic |

---

## Phase 2: SchedulerHandle

**File**: `src/lib.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `handle_new` | `SchedulerHandle::new(token)` stores token |
| `handle_cancel_sets_flag` | `handle.cancel()` → `handle.is_cancelled()` returns `true` |
| `handle_not_cancelled_initially` | `handle.is_cancelled()` → `false` before cancel |
| `handle_token_accessor` | `handle.token()` returns the underlying `CancellationToken` |
| `handle_clone` | Cloned handle shares cancellation state |

---

## Phase 3: Interval Execution (Integration)

**File**: `tests/interval_test.rs` (new integration test)

All tests use `#[tokio::test(flavor = "multi_thread")]` with short intervals (50-200ms).

| Test | Description |
|------|-------------|
| `interval_task_runs_repeatedly` | Task with 100ms interval runs at least 3 times in 400ms |
| `interval_task_stops_on_cancel` | Cancel token after 250ms → task stops running |
| `interval_with_initial_delay` | `IntervalWithDelay { interval: 100ms, initial_delay: 200ms }` → first run after ~200ms |
| `interval_task_state_accessible` | Captured `T` state accessible inside task closure |
| `interval_task_error_continues` | Task returning `Err` → logs and continues next iteration |
| `interval_task_panic_doesnt_crash_scheduler` | Panic in task → other tasks unaffected |

---

## Phase 4: Cron Execution (Integration)

**File**: `tests/cron_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `cron_task_parses_valid_expression` | Valid cron expression → task starts without error |
| `cron_task_invalid_expression_logs_error` | Invalid cron → logs error, returns early (no panic) |
| `cron_task_stops_on_cancel` | Cancel token → cron task stops |
| `cron_task_runs_at_scheduled_time` | `"* * * * * *"` (every second) → runs at least once in 2s |

---

## Phase 5: Task Registry & Lifecycle

**File**: `tests/lifecycle_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `extract_tasks_from_boxed` | `extract_tasks(vec![...])` correctly downcasts `ScheduledTaskDef` |
| `extract_tasks_empty` | `extract_tasks(vec![])` returns empty vec |
| `multiple_tasks_all_start` | 3 tasks registered → all 3 running |
| `task_name_accessible` | `ScheduledTask::name()` returns configured name |
| `task_schedule_accessible` | `ScheduledTask::schedule()` returns configured schedule |

---

## Phase 6: State Capture & Isolation

**File**: `tests/state_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `state_cloned_for_task` | Task captures `Clone` state independently |
| `concurrent_tasks_independent_state` | Two tasks with `Arc<AtomicUsize>` → independent counters |
| `state_mutations_visible_via_arc` | Shared `Arc<Mutex<_>>` state → mutations visible across runs |

---

## Phase 7: Plugin Integration

**File**: `tests/plugin_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `scheduler_plugin_provides_token` | `Scheduler` plugin → `CancellationToken` available in state |
| `scheduler_plugin_deferred_setup` | Deferred action stores `TaskRegistryHandle` |
| `full_lifecycle` | Plugin install → build state → register controller → serve → tasks running → cancel → tasks stopped |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 6 | 1h | None |
| Phase 2 | 5 | 1h | None |
| Phase 3 | 6 | 3h | Timing-sensitive |
| Phase 4 | 4 | 2h | cron crate |
| Phase 5 | 5 | 2h | r2e-core types |
| Phase 6 | 3 | 1.5h | None |
| Phase 7 | 3 | 2h | Full AppBuilder setup |
| **Total** | **32** | **~12.5h** | |

---

## Notes

- Timing-sensitive tests should use generous margins (2-3x expected duration) to avoid flaky CI.
- Consider a `tokio::time::pause()` approach for deterministic timing tests where possible.
- All integration tests need `tokio = { features = ["full", "test-util"] }` as dev-dependency.
