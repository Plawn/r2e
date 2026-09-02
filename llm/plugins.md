---
topic: plugins
features: core
tokens: ~2800
requires: di-beans
---

## Plugins

### TL;DR

- One plugin kind (`Plugin`), one install call — `.plugin(p)`, always **before**
  `build_state()`. There is no `install`/`configure` split.
- Implement four associated types: `Provided` (tuple of beans, one bean each),
  `Deps` (any bean, delivered by value, order-independent, verified at
  `build_state()`), `Config`, `Controllers`.
- `build` is async and fallible and runs as a node of the bean graph: an `Err`
  aborts boot.
- `build` **always** runs, even when disabled — check `ctx.enabled()` before any
  global side effect and return a cheap **inert** variant when it is false.
- Register effects on `PluginBuildContext` by stage: `add_layer` / `after_build` /
  `store_data` / `on_serve` (Graph), `after_routes` (Routes — runs once **every**
  controller is registered, so install order is irrelevant), `wrap_router`
  (Finalize, outermost).
- Disabling drops the surface stages but **not** `on_shutdown` /
  `on_shutdown_async`: what `build` constructed must still be released.
- Put the rare pre-graph needs in `fn setup(&mut self, &mut PluginSetupContext)` —
  setup actions are never gated on `<prefix>.enabled`, and surface sugar
  (`add_layer`, `wrap_router`, serve/shutdown hooks) there is a compile error.
- Start serve-time work with `serve_ctx.track(fut)` (a **future**, not a handle),
  never a bare `rt::spawn` — tracking is what keeps the graph alive and joins the
  work at shutdown.
- An effect needing one of the plugin's own beans must resolve it from the graph at
  apply time (`after_build(|dctx| dctx.bean_context().try_get::<X>())`) instead of
  capturing what `build` made.
- Ship endpoints with `type Controllers = (..)`; contribute a health probe by
  putting `HealthRegistry` in `Deps` (which makes installing `AdvancedHealth` a
  compile-time requirement).

There is exactly **one** plugin kind (`Plugin`) and **one** install call
(`.plugin(p)`, always before `build_state()`). A plugin IS one async, fallible
factory for its `Provided` tuple:
`build` runs inside `build_state()` as a node of the bean graph, after `Deps`
(real topological edges, delivered by value), with typed config guaranteed
loaded. There is no `install`/`configure` split and no shell/fill dance.

```rust
pub struct MyPlugin;

impl Plugin for MyPlugin {
    type Provided = (MyHandle,);          // tuple of beans; each becomes its own bean
    type Deps = (SqlitePool,);            // ANY bean (.provide()-d or .register()-ed),
                                          // verified at build_state(), order-independent
    type Config = ();                     // or a ConfigProperties struct + CONFIG_PREFIX
    type Controllers = ();                // or a tuple of #[controller] types the plugin ships

    async fn build(self, (pool,): Self::Deps, _config: Option<()>,
                   ctx: &mut PluginBuildContext)
        -> Result<Self::Provided, PluginBuildError> {
        let handle = MyHandle::connect(pool).await?;   // async + fallible: Err aborts boot
        let h = handle.clone();
        ctx.on_shutdown_async(move || async move { h.drain().await });
        Ok((handle,))
    }
}
# fn main() {}
```

`PluginBuildContext`: `enabled()` (the `<prefix>.enabled` gate — build ALWAYS
runs, return a cheap INERT disabled variant when false, checking the gate before
any global side effect; the gate is decided once here, not re-read later),
`graph() -> GraphHandle` (handle on the final resolved graph, for request-time
lookups), `config_raw()`, `add_layer`, `after_routes`, `wrap_router`,
`store_data`, `on_serve`, `on_serve_each_cycle` (an `on_serve` that also runs
on `r2e dev` hot-patch cycles — for transports serving their own port; bind
through `ServeContext::bind_tcp(owner, addr)` (async → `BoundListener`) so the
socket carries over between cycles, and serve through
`bound.into_incoming(shutdown)` — a stream that stops before any accept on
shutdown or handover and releases the socket (so does `stop_signal` when it
resolves); the next cycle's `bind_tcp` waits for that release so the old
server normally does not accept after the new one starts — bounded to 5 s: a
holder that never releases is overridden with a warning and may still race for
queued connections),
`on_shutdown`, `on_shutdown_async`, `after_build` (full-graph boot escape
hatch).

**Three effect stages**, applied in this order (install order *within* a stage;
builds themselves run in dependency order):

| Stage | Registered with | When it applies |
|---|---|---|
| Graph | `add_layer`, `after_build`, `store_data`, `on_serve`, `on_serve_each_cycle` | during `build_state()` / router assembly; a later `add_layer` is the outer one |
| Routes | `after_routes(FnOnce(&mut RoutesContext))` | in `build()`, once **every** controller (app, module and plugin-shipped) is registered — so install order is irrelevant |
| Finalize | `wrap_router` | last, outermost (transport-level wrappers) |

`RoutesContext` gives `routes() -> &[RouteInfo]` (the collected route registry —
what `OpenApiPlugin` reads), `register_routes(Router)`, `bean_context()`,
`config()`, `take_data::<D>()`. Nothing has to be "installed last" any more.

Disabling drops all three **surface** stages (`add_layer`, `after_routes`,
`wrap_router`, `on_serve`, `store_data`, `after_build`) but NOT the cleanup hooks
(`on_shutdown`, `on_shutdown_async`): `build` ran, so what it constructed must
still be released. An effect that needs one of the plugin's own beans should resolve it
from the graph at apply time (`after_build(|dctx| dctx.bean_context()
.try_get::<X>())`) instead of capturing what `build` made — otherwise a test
that pins only *some* provisions leaves the effect talking to an instance the
graph does not expose. Rare pre-graph needs (store_data other subsystems read
even when disabled, `run_pre_destroy`, raw `add_deferred`) go in the optional
`fn setup(&mut self, &mut PluginSetupContext)` — setup actions are NEVER gated
on `<prefix>.enabled` (that is why a disabled Scheduler still collects
`#[scheduled]` tasks), which is exactly why `PluginSetupContext` has no surface
*sugar*: no `add_layer`, no `wrap_router`, no serve/shutdown hooks (using one is
a compile error). `add_deferred` remains as the raw, explicitly **unconditional**
escape hatch — it hands you the whole `DeferredContext`, so what you do there
runs even when the plugin is disabled; put anything that should disappear under
`enabled = false` in `build`. Plugin beans register strictly: same-type app
bean or a double install = `DuplicateBean` at boot; in tests, `override_bean`
each provided type BEFORE `.plugin()` — the pins win per type and `build` still
runs, so the plugin's routes/layers/hooks stay mounted. A plugin whose `build`
is pure bean construction (no effects) and expensive may opt out with
`const SKIP_BUILD_WHEN_ALL_PINNED: bool = true;` (default `false`; `OpenFga`
sets it) — then pinning every provided type skips `build` entirely. To silence
an effect-carrying plugin in a test, set `<prefix>.enabled = false`.

`GraphHandle` holds a **weak** reference to the resolved `BeanContext` (it
usually lives inside that same graph, so a strong one would leak it). It is
filled on every *successful* `build_state()`. Three independent owners keep it
alive: (1) the assembled router — the strong reference rides each request future
AND its response body, so the graph outlives every in-flight request even when
the router itself was already dropped (`router.oneshot(req)` hands the service
to the future before polling it); (2) **every tracked task** — `ServeContext::
track`, `spawn_service`, the scheduler driver, the QUIC drain and every upgraded
WebSocket session move an `Arc` *into* the task, which is what makes them sound on the paths where nothing joins
the handle (an elapsed per-handle `shutdown_grace_period`, a dropped `run()` future under
`r2e dev` — where they are still cancelled, since every framework shutdown token
is a child of the app one, or relayed from it as the scheduler's is); (3) the app
object itself for the whole of its lifetime — `PreparedApp` across `run()` when
serving, `RunningApp` across a `TestApp`'s life in process — which covers the
shutdown phase (`on_stop`, `#[pre_destroy]`). Serve-time work
must therefore be started through `track`, never a bare `rt::spawn`:

```rust
# fn __doc(dctx: &mut DeferredContext) {
dctx.on_serve(move |serve_ctx| {
    let shutdown = serve_ctx.shutdown_token();   // r2e::rt::CancelToken
    serve_ctx.track(async move { my_server(shutdown).await; });  // future, not a handle
});
# }
```

`run()` cancels the shutdown token and drains those handles on every exit it
controls, including an aborted boot (a startup hook returning `Err`, a serve
error) — user `on_drain`/`on_stop` hooks and `#[pre_destroy]` disposers do not
run on that abort path. A WebSocket session is detached from its connection
(hyper completes an upgraded connection immediately), so it is put back in view
explicitly: sessions from `#[ws(...)]` routes run on the tracked lane, own the
graph while they run, and are joined in the same phase as the other tracked
handles. Only a router served outside `run()` (`TestApp`, a hand-rolled
`axum::serve`) leaves them detached and graph-less.
`get()`/`bean::<B>()` return `Some` for the app's lifetime and
`None` after it and everything in flight are gone (or after a failed boot) —
never a panic.
`r2e::Late<T>` (write-once, Arc-shared cell) remains only as an escape hatch
for genuinely post-boot fills.

### Plugin-shipped controllers

`type Controllers = (MyPluginController,)` — a plugin ships endpoints the normal
way (`#[controller]` + `#[routes]`). They are registered by the builder like app
controllers, with the same `EndpointDeps`/`AllSatisfied` compile check: a
controller injecting a bean the plugin does not provide (and the app does not
either) is a **compile error**, not a boot panic. Plugin controllers are visible
to `after_routes` (Routes stage), so `OpenApiPlugin` documents them too.

### `HealthRegistry` — plugins contributing health checks

`Health` (simple `/health` → 200 "OK") provides nothing. `Health::builder()
.check(..).build()` → `AdvancedHealth`, which serves `/health`, `/health/live`,
`/health/ready` and provides `HealthRegistry`. Any other plugin publishes a
probe by declaring it as a dependency:

```rust
use r2e::builtins::health::HealthRegistry;

pub struct DbHealth;

impl Plugin for DbHealth {
    type Provided = ();
    type Deps = (SqlitePool, HealthRegistry);   // installing AdvancedHealth is now a compile-time requirement
    type Config = ();
    type Controllers = ();

    async fn build(self, (pool, health): Self::Deps, _c: Option<()>,
                   _ctx: &mut PluginBuildContext) -> Result<(), PluginBuildError> {
        health.register(PingIndicator { pool });   // order vs AdvancedHealth is irrelevant
        Ok(())
    }
}
# fn main() {}
```

In-tree: `DataSourceHealth<DB, Tag>` (`r2e-data-sqlx`) / `DataSourceHealth<Conn,
Tag>` (`r2e-data-diesel`) run a `SELECT 1` against the pool;
`.liveness_only()` keeps a check out of readiness.
