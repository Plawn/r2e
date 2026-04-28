# R2E Tech Debt

Rolling list of framework-level items deferred from tasks so they don't get lost. Add new items as they surface, move them to `## Shipped` (with commit/tag) when resolved.

## Current

### `r2e-executor` — replace `JobHandle<T>` with `tokio::task::JoinHandle<T>`

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **What:** `JobHandle<T>` wraps an extra `oneshot::Receiver` per submission and collapses panic / abort / shutdown into a single `JobError::Cancelled` variant. `tokio::task::JoinHandle<T>` already implements `Future<Output = Result<T, JoinError>>` and preserves panic / cancellation distinction (`is_panic()`, `is_cancelled()`, `into_panic()`).
- **Why deferred:** changes the public return type of `submit` / `try_submit` and of every `#[async_exec]`-generated wrapper.
- **Ref:** `r2e-executor/src/lib.rs` (`JobHandle`, `spawn_job`).

### `r2e-executor` — async `on_shutdown` so graceful drain is bounded

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **What:** the executor's shutdown hook spawns `shutdown_graceful(timeout)` as a fire-and-forget task because `DeferredContext::on_shutdown` is sync-only (`FnOnce() + Send`). If the runtime tears down before the spawned task completes, the configured timeout is not actually honored.
- **Fix:** add an async-capable shutdown hook to `r2e-core/src/plugin.rs` (`on_shutdown_async<F: FnOnce() -> BoxFuture<'static, ()>>`) and have the builder await it inside `shutdown_future`. Then route the executor drain through it.
- **Ref:** `r2e-executor/src/lib.rs` (Executor plugin install), `r2e-core/src/plugin.rs` (`DeferredContext::on_shutdown`), `r2e-core/src/builder.rs` (~line 1500, `shutdown_future`).

### `r2e-macros` — share field-resolution walker across `Controller` / `Bean` / `BackgroundService` derives

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **What:** `bg_service_derive.rs`, `derive_codegen.rs` (`generate_stateful_construct`), and `bean_derive.rs` each walk struct fields and emit nearly identical `field_inits` for `#[inject]` / `#[config]` / `#[config_section]`. Error messages and panic strings have already drifted slightly between them.
- **Fix:** factor a `walk_injected_fields(&fields, &state_type) -> Vec<TokenStream>` helper (e.g. in `type_utils.rs` or a new `field_resolver.rs`) and rewrite all three derives on top of it.
- **Ref:** `r2e-macros/src/{bg_service_derive.rs,derive_codegen.rs,bean_derive.rs}`.

### `r2e-executor` — drop `running` atomic in favor of `Semaphore::available_permits()`

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **What:** `running` duplicates state already implied by the semaphore (`max_concurrent - sem.available_permits()`). Two atomics back what is essentially one piece of state — drift risk on every future change.
- **Why deferred:** post-shutdown semantics differ (the closed semaphore reports 0 available permits regardless of in-flight tasks); needs a dedicated drain counter (`JoinSet` / `Notify` + count-down) to replace the metric.
- **Ref:** `r2e-executor/src/lib.rs` (`Inner::running`, `metrics()`, `shutdown_graceful`).

### `r2e-executor` — typed `Duration` and `u64` config fields

- **Surfaced:** 2026-04-28 (`/simplify` review of Tasker #190).
- **What:** `ExecutorConfig` uses `i64` for `max_concurrent`, `queue_capacity`, `shutdown_timeout_secs`. Negative values are silently coerced via `.max(0)`; the timeout is in raw seconds with no unit in the type. Switching to `u64` (capacities) and `Duration` (timeout via `humantime`-style `FromConfigValue`) lets users write `shutdown-timeout: 30s`.
- **Why deferred:** affects `ConfigProperties` surface — same pattern is used across `r2e-scheduler`, `r2e-cache`, etc., so the right fix is a workspace-wide `Duration` `FromConfigValue` impl, not a per-crate change.
- **Ref:** `r2e-executor/src/lib.rs` (`ExecutorConfig`).

## Shipped

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
