---
topic: lifecycle-hooks
features: core
tokens: ~2000
requires: di-beans
---

## Lifecycle Hooks — `#[post_construct]`, `#[pre_destroy]`, `#[on_start]`

### TL;DR

- The three hooks share one signature shape: `&self` only, sync or async,
  returning `()` or `Result<(), Box<dyn Error + Send + Sync>>`. Parameters are a
  compile error.
- All three go on `#[bean]` impls **and** on `#[routes]` controller impls.
- `#[post_construct]` — bean hooks run inside `build_state()`; controller-core
  hooks run at startup during `register_controller` / `build_with_consumers`,
  before that app's consumer registrations. An `Err` aborts startup.
- `#[on_start]` — the late sibling: runs once the whole graph **and every
  controller core** exist, after consumer registrations and before the server
  binds. An `Err` aborts boot.
- `#[on_start(order = N)]` orders hooks globally (`i32`, default `0`, ascending,
  ties in registration order); `#[on_start(once)]` runs on the first startup of
  the process only — use it for work that must not repeat on an `r2e dev` hot patch.
- `#[pre_destroy]` — runs at graceful shutdown: controller hooks first, then bean
  hooks, each in **reverse registration order**. An `Err` is logged and swallowed;
  disposal never aborts shutdown.
- A pinned `override_bean` **skips** `#[on_start]` and `#[pre_destroy]`.
- Combining any of these with a route / `#[scheduled]` / `#[consumer]` /
  `#[async_exec]` / `#[intercept]` — or with each other — on one method is a
  compile error.
- For values entering the graph via `.provide(instance)` there is no attribute:
  use `AppBuilder::provide_with_pre_destroy(value)`, or `ctx.run_pre_destroy::<T>()`
  in a plugin.
- All of this runs under `TestApp::boot` / `build_with_consumers` too (a test boot
  is a real startup); disposal fires on `app.shutdown().await`.

`#[post_construct]` runs after the full bean graph is resolved. `&self` only,
sync or async, returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`:

```rust
#[derive(Clone)]
pub struct CacheService { pool: SqlitePool }

#[bean]
impl CacheService {
    pub async fn new(pool: SqlitePool) -> Self { Self { pool } }

    #[post_construct]
    async fn warm_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}
```

`#[post_construct]` also works on **controller** `#[routes]` impls (same
signature rules). Timing differs: bean hooks run inside `build_state()`;
controller-core hooks run at startup during `register_controller` /
`build_with_consumers`, **before** that app's consumer registrations. An `Err`
aborts startup. On a controller it is a compile error to put `#[post_construct]`
on a route / `#[scheduled]` / `#[consumer]` method, to give it params, or to add
`#[intercept]` to it.

```rust
#[controller]
pub struct LifecycleController {
    #[inject] event_bus: LocalEventBus,
}

#[routes]
impl LifecycleController {
    #[post_construct]
    async fn init(&self) { /* runs once at startup, before consumers */ }

    #[consumer(bus = "event_bus")]
    async fn on_ping(&self, _e: Arc<Ping>) { /* ... */ }
}
# fn main() {}
```

### `#[pre_destroy]` — disposal hooks

The `@PreDestroy` counterpart of `#[post_construct]`, on `#[bean]` impls AND
`#[routes]` controller impls. Same signature rules (`&self` only, sync or async,
returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`). Runs during
**graceful shutdown**, in the async shutdown phase — controller hooks first, then
bean hooks, each in **reverse registration order** (a controller/bean disposes
before the beans it injected). An `Err` is **logged and swallowed** — disposal
never aborts shutdown. A pinned `override_bean` **skips** the hook (same
undecorated-pin rule as `#[scheduled]`/`#[post_construct]`). Same rejection
matrix as `#[post_construct]`: it is a compile error to combine `#[pre_destroy]`
with a route / `#[scheduled]` / `#[consumer]` / `#[async_exec]` /
`#[post_construct]` / `#[intercept]` on one method, or to give it parameters.

```rust
#[derive(Clone)]
pub struct ConnectionPool { cfg: PoolConfig }

#[bean]
impl ConnectionPool {
    pub fn new(cfg: PoolConfig) -> Self { Self { cfg } }

    #[pre_destroy]
    async fn close(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.drain_and_close().await?;   // Err is logged, shutdown continues
        Ok(())
    }

    async fn drain_and_close(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

#[controller]
pub struct LifecycleController;

#[routes]
impl LifecycleController {
    #[pre_destroy]
    async fn flush(&self) { /* runs once at shutdown */ }
}
# fn main() {}
```

For values entering the graph via `.provide(instance)` (or plugin `Provided`
beans), use the imperative form `AppBuilder::provide_with_pre_destroy(value)`
(the value implements the `PreDestroy` trait) or, in a plugin's `setup`,
`ctx.run_pre_destroy::<T>()`.

In tests, disposal fires on `app.shutdown().await` — `TestApp` runs the same
shutdown sequence as `run()` (see llm/testing.md).

### `#[on_start]` — startup observers

The bean/controller counterpart of the builder's `.on_start(closure)`, and the
**late** sibling of `#[post_construct]`: a `#[post_construct]` hook runs *inside*
`build_state()` while the graph is still being assembled, so it cannot observe
controllers or anything registered after it; an `#[on_start]` hook runs once the
whole graph **and every controller core** exist, and before the server binds.
Works on `#[bean]` impls (via the `OnStart` trait, read by value from the
resolved graph — a pinned `override_bean` **skips** the hook) and on `#[routes]`
controller impls (run from the core `Arc`). Same signature rules as the two hooks
above (`&self` only, sync or async, returns `()` or
`Result<(), Box<dyn Error + Send + Sync>>`). An `Err` **aborts boot**, exactly
like an `Err` from an `.on_start(…)` closure.

`#[on_start(order = N)]` sets the run order (`i32`, default `0`, ascending; ties
keep registration order — bean hooks before controller hooks, each in declaration
order). Ordering is **global**: every `#[on_start]` hook in the application is
sorted into one list. The hooks run after the consumer registrations and before
the plugin serve hooks and the builder's `.on_start(…)` closures.

`#[on_start(once)]` runs the hook on the **first startup of the process** only.
It exists for `r2e dev`: a hot patch re-assembles the application in the same
process, and work like crash recovery or claiming a lock must not repeat.
Outside `r2e dev` it is identical to a plain `#[on_start]` (one boot = one
cycle). A patch that *changes* the hook body does **not** re-run it; a boot that
failed before finishing its hook phase has consumed nothing, so the next cycle
retries. `once` composes with `order` in either spelling —
`#[on_start(once, order = -10)]`. The builder equivalent is `.on_start_once(…)`.

These hooks run under `TestApp::boot` and `build_with_consumers` too (a test
boot is a real startup) — on the router-only `build_with_consumers` path an
`Err` panics instead of aborting `serve`; `TestApp::try_boot*` returns it as a
`BootError`.

Same rejection matrix as the other two hooks: it is a compile error to combine
`#[on_start]` with a route / `#[scheduled]` / `#[consumer]` / `#[sse]` / `#[ws]`
/ `#[async_exec]` / `#[post_construct]` / `#[pre_destroy]` / `#[intercept]` on
one method, or to give it parameters.

```rust
#[derive(Clone)]
pub struct WarmCache { store: Store }

#[bean]
impl WarmCache {
    pub fn new(store: Store) -> Self { Self { store } }

    #[on_start(order = -10)]
    async fn preload(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.store.load().await?;      // Err aborts boot
        Ok(())
    }
}

#[controller]
pub struct LifecycleController;

#[routes]
impl LifecycleController {
    #[on_start]
    async fn announce(&self) { /* every controller core exists here */ }
}
# fn main() {}
```
