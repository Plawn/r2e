# r2e-events — Test Development Plan

## Current State

- **6 tests** (all inline in `src/lib.rs`)
- **Coverage**: ~55% — core pub/sub and concurrency control tested
- **Gap**: Error/panic handling, concurrent subscription safety, handler lifecycle, re-entrancy

---

## Phase 1: Error & Panic Isolation (Critical)

**File**: `src/lib.rs` — extend `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `handler_panic_does_not_crash_emit` | Handler panicking in `emit()` → bus still functional |
| `handler_panic_does_not_crash_emit_and_wait` | Handler panicking in `emit_and_wait()` → returns without panic |
| `panic_releases_permit` | Handler panic with concurrency=1 → permit released, next handler runs |
| `multiple_handlers_one_panics` | 3 handlers, 1 panics → other 2 still execute |
| `err_result_in_handler` | Handler returning `Err` internally → no effect on bus |

---

## Phase 2: Subscription Safety

| Test | Description |
|------|-------------|
| `late_subscriber_misses_event` | Subscribe after `emit()` → does not receive past event |
| `concurrent_subscribes` | 10 threads subscribing simultaneously → all registered |
| `subscribe_during_emit` | `subscribe()` while `emit()` is in-flight → no panic, consistent state |
| `subscribe_same_event_type_multiple` | Multiple subscriptions for same `TypeId` → all called |

---

## Phase 3: Edge Cases & Lifecycle

| Test | Description |
|------|-------------|
| `emit_no_subscribers` | `emit()` on bus with zero subscribers → instant, no error |
| `emit_and_wait_no_subscribers` | `emit_and_wait()` with zero subscribers → instant return |
| `default_eventbus` | `EventBus::default()` equivalent to `EventBus::new()` |
| `concurrency_limit_bounded` | `EventBus::with_concurrency(5).concurrency_limit()` → `Some(5)` |
| `concurrency_limit_unbounded` | `EventBus::unbounded().concurrency_limit()` → `None` |
| `clone_shares_state` | Cloned bus shares subscribers with original |
| `drop_bus_with_active_handlers` | Drop `EventBus` while handlers running → no panic |

---

## Phase 4: Async Handler Behavior

| Test | Description |
|------|-------------|
| `handler_with_long_sleep` | `emit()` returns immediately despite slow handler |
| `emit_and_wait_waits_for_slow` | `emit_and_wait()` blocks until slow handler completes |
| `handler_spawns_nested_emit` | Handler calling `bus.emit()` internally → no deadlock |
| `handler_shared_state_mutation` | Multiple handlers modifying `Arc<Mutex<_>>` → consistent final state |

---

## Phase 5: Stress & Performance

| Test | Description |
|------|-------------|
| `stress_many_events` | Emit 1000 events → all delivered to subscriber |
| `stress_many_subscribers` | 100 subscribers → all receive event |
| `stress_concurrent_emit` | 10 threads emitting simultaneously → no data loss |
| `backpressure_high_load` | Concurrency=2 with 50 events → max 2 concurrent at any time |

---

## Phase 6: Consumer Integration (via example-app)

**File**: `example-app/tests/consumer_test.rs` (new)

| Test | Description |
|------|-------------|
| `consumer_method_invoked` | Emit event → `#[consumer]` method called |
| `consumer_receives_correct_data` | Event payload accessible in consumer |
| `consumer_with_injected_deps` | Consumer uses `#[inject]` fields from state |
| `multiple_consumers_same_event` | Two controllers consuming same event type → both invoked |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 5 | 1.5h | None |
| Phase 2 | 4 | 1.5h | None |
| Phase 3 | 7 | 1h | None |
| Phase 4 | 4 | 1.5h | None |
| Phase 5 | 4 | 1h | None |
| Phase 6 | 4 | 2h | example-app fixtures |
| **Total** | **28** | **~8.5h** | |
