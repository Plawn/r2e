# R2E Tech Debt

Rolling list of framework-level items deferred from tasks so they don't get lost. Add new items as they surface, move them to `## Shipped` (with commit/tag) when resolved.

## Current

<!-- No active items. -->

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
