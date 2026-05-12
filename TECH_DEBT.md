# R2E Tech Debt

Rolling list of framework-level items deferred from tasks so they don't get lost. Add new items as they surface, move them to `## Shipped` (with commit/tag) when resolved.

## Current

(none)

## Shipped

### `r2e-executor` — replace `JobHandle<T>` with `tokio::task::JoinHandle<T>`

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **Landed:** 2026-05-12 (tech-debt batch).
- **What:** removed `JobHandle<T>` / `JobError` entirely. `submit` / `try_submit` now return `Result<JoinHandle<T>, RejectedError>`. `#[async_exec]`-generated wrappers updated. Callers get native `JoinError` panic/cancellation distinction.
- **Ref:** `r2e-executor/src/lib.rs`, `r2e-macros/src/codegen/wrapping.rs`.

### `r2e-executor` — async `on_shutdown` so graceful drain is bounded

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **Landed:** 2026-05-12 (tech-debt batch).
- **What:** added `DeferredContext::on_shutdown_async` and `AsyncShutdownHook` type to `r2e-core/src/plugin.rs`. Builder awaits async hooks inside `shutdown_future`. Executor plugin now uses `on_shutdown_async` so graceful drain is properly bounded by the configured timeout.
- **Ref:** `r2e-core/src/plugin.rs`, `r2e-core/src/builder.rs`, `r2e-executor/src/lib.rs`.

### `r2e-macros` — share field-resolution walker across `Controller` / `Bean` / `BackgroundService` derives

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **Landed:** 2026-05-12 (tech-debt batch).
- **What:** added `r2e-macros/src/field_resolver.rs` with `classify_fields()`, `ClassifiedField`, `FieldKind`, `config_init_panic()`, `config_section_init_panic()`. Rewrote `bg_service_derive.rs` and `derive_codegen.rs` (`generate_stateful_construct`) on top of the shared walker. `bean_derive.rs` uses `classify_fields` for field classification but keeps its own init code (different `BeanContext` resolution path).
- **Ref:** `r2e-macros/src/field_resolver.rs`, `r2e-macros/src/bg_service_derive.rs`, `r2e-macros/src/derive_codegen.rs`.

### `r2e-executor` — drop `running` atomic in favor of `Semaphore::available_permits()`

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **Landed:** 2026-05-12 (tech-debt batch).
- **What:** removed `running: AtomicU64` from `Inner`. `metrics().running` now derives from `max_concurrent - semaphore.available_permits()` when open, or `drain_count` (new `AtomicU64`) when shut down. `try_submit` backpressure also uses the semaphore-derived count.
- **Ref:** `r2e-executor/src/lib.rs`.

### `r2e-executor` — typed `Duration` and `u64` config fields

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **Landed:** 2026-05-12 (tech-debt batch).
- **What:** `ExecutorConfig` fields changed from `i64` to `u64` (capacities) and `Duration` (timeout). Added `impl FromConfigValue for std::time::Duration` to `r2e-core` supporting integer-as-seconds and string suffixes (`s`, `ms`, `m`/`min`, `h`/`hr`). Config key changed from `shutdown-timeout-secs` to `shutdown-timeout`; users can write `shutdown-timeout: 30s`.
- **Ref:** `r2e-core/src/config/value.rs`, `r2e-executor/src/lib.rs`.

### `WsRooms` / `SseRooms` — share implementation

- **Landed:** 2026-04-11 as part of Tasker #77 follow-up (deferred item cleanup).
- **What:** `WsRooms` was generalized to `WsRooms<K = String>` with the same `Borrow<Q>` `remove` signature as `SseRooms`. Both types now mirror each other verbatim — the shared bodies are short enough that extracting a macro/helper would add more indirection than it saves.
- **Ref:** `r2e-core/src/ws.rs` (`WsRooms`), `r2e-core/src/sse.rs` (`SseRooms`).

### `WsRooms` / `SseRooms` — unbounded growth

- **Landed:** 2026-04-11 as part of Tasker #77 follow-up.
- **What:** both registries now have a `reap_empty(&self) -> usize` helper that drops rooms whose broadcaster has zero active subscribers, returning the number of rooms removed. No background reaper — callers schedule it via `r2e-scheduler` if they want automatic cleanup.
- **Ref:** `r2e-core/src/ws.rs` (`WsRooms::reap_empty`), `r2e-core/src/sse.rs` (`SseRooms::reap_empty`). Tests: `tests/ws.rs::rooms_reap_empty_drops_subscriberless_rooms`, `tests/sse.rs::sse_rooms_reap_empty_drops_subscriberless_rooms`.

### `SseBroadcaster` — `LagPolicy` enum

- **Landed:** 2026-04-11 as part of Tasker #77 follow-up.
- **What:** promoted the internal `Option<String>` on `SseSubscription` to a public `#[non_exhaustive] enum LagPolicy { Silent, Synthetic(String) }`. Added `SseBroadcaster::subscribe_with(policy)` and `SseRooms::subscribe_with(key, policy)` as the underlying primitives; the existing `subscribe` / `subscribe_lagged` methods are kept as ergonomic shortcuts that delegate. `LagPolicy` is exported from the prelude.
- **Ref:** `r2e-core/src/sse.rs` (`LagPolicy`, `SseBroadcaster::subscribe_with`), `r2e-core/src/prelude.rs`. Test: `tests/sse.rs::sse_subscribe_with_policy_delegates`.

### `r2e-core::managed::ManagedError` deprecation warnings

- **Landed:** 2026-04-11 as part of Tasker #77 follow-up.
- **What:** deleted the deprecated `ManagedError` tuple struct and all its trait impls from `r2e-core/src/managed.rs`. Migrated the sole workspace consumer (`r2e-data-sqlx/src/tx.rs`) and the `r2e-core/tests/managed.rs` regression test to `ManagedErr<HttpError>`. Removed the re-exports from `r2e-core/src/lib.rs` and `r2e-core/src/prelude.rs`, and updated all docs (`README.md`, `docs/book/**`, `docs/claude/**`, `llm.txt`, `TEST_DEVELOPMENT_PLAN.md`, `REPO_MAP.md`). `cargo check -p r2e-core` now produces zero deprecation warnings for this crate.
- **Ref:** `r2e-core/src/managed.rs`, `r2e-data-sqlx/src/tx.rs`, `r2e-core/tests/managed.rs`.
