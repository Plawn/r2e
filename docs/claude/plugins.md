# Plugin System Reference

Authoritative reference for R2E's plugin system as of the **factory-first
redesign (W15, 2026-08-15)**: `PreStatePlugin` is one async, fallible factory
(`build`) executed inside `build_state()` as a node of the bean graph. The old
two-phase `install`/`configure` machine — and the `Late<T>` shell-then-fill
dance it forced — is gone. Source of truth: `r2e-core/src/plugin.rs`,
`r2e-core/src/type_list.rs` (`PluginDeps` / `PluginProvisions`),
`r2e-core/src/config/mod.rs` (`PluginConfig`), `r2e-core/src/di/late.rs`
(`Late<T>`, now an escape hatch only).

## Two plugin kinds

| | Pre-state | Post-state |
|---|---|---|
| Trait | `PreStatePlugin` | `Plugin` |
| Install call | `.plugin(p)` **before** `build_state()` | `.with(p)` **after** `build_state()` |
| Can provide beans | yes (tuple `Provided`) | no |
| Typical use | Scheduler, Prometheus, OIDC, gRPC, Executor, Tenancy, OpenFga | Health, Cors, OpenApi, NormalizePath |

Passing one to the other's install method is a guided compile error
(`#[diagnostic::on_unimplemented]` on `Plugin`, `PreStatePlugin`, and
`RawPreStatePlugin`). `Plugin` also has advisory `should_be_last()` — the
builder warns if another post-state plugin is added after one that returns
`true` (e.g. `NormalizePath`).

**Pre-state plugins only run on the graph path.** `build_state().await.build()`
(or `serve*`) executes plugin builds; the legacy `with_state(())` shortcut
throws the bean registry away, so the group node never runs: no `build`, no
provisions, no layers/routes/serve hooks, and no cleanup hooks either. What
*does* still run there is `setup()` and the deferred actions it queued
(`store_data`, `add_deferred`) — they are pre-graph by construction. The
plugin's effect action then finds its slot empty and logs a `debug!` no-op;
`.plugin(P).with_state(())` is a supported (if pointless) combination, not a
panic. Tests exercising a plugin's router surface must go through
`build_state()`.

## PreStatePlugin surface

```rust
impl PreStatePlugin for MyPlugin {
    type Provided = (MyService,);          // tuple: (A,), (A, B), or () — never a bare type
    type Deps     = (DbPool, PoolExecutor); // real topo edges; arrive built, by value
    type Config   = MyConfig;              // or (); #[derive(ConfigProperties)] section
    const CONFIG_PREFIX: Option<&'static str> = Some("my-plugin");
    // const BUILD_VERSION: u64 = 0;       // optional dev-reload stamp, rarely needed

    // Rare pre-graph escape hatch (default no-op) — see "setup()" below.
    fn setup(&mut self, ctx: &mut PluginSetupContext) {}

    // THE plugin: one async fallible factory for `Provided`.
    async fn build(
        self,                              // by value: builder fields still on self
        (pool, executor): Self::Deps,      // constructed BEFORE build (topo order)
        config: Option<Self::Config>,      // None if section absent; parsed + validated if present
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let svc = MyService::connect(pool, config.unwrap_or_default()).await?; // Err aborts boot
        let h = svc.handle();
        ctx.on_shutdown_async(move || async move { h.drain().await });
        Ok((svc,))
    }
}
```

`PluginBuildError = Box<dyn std::error::Error + Send + Sync>`. An `Err` from
`build` aborts startup as `BeanError::PluginBuild { plugin, source }`
(Display: ``Plugin '<name>' failed to build: <source>``); `build_state()`
panics with `Failed to resolve bean dependency graph: <that message>`, so
`#[should_panic(expected = "...")]` substring tests work.

### Lifecycle

```
.plugin(Me)                          build_state()                        (serve)
     │                                    │                                  │
     ▼                                    ▼                                  ▼
 setup(&mut self)          graph resolution (topological):             on_serve hooks
   registry node queued      deps built → build(deps, config, ctx)
                           then effects applied, per plugin,
                           in INSTALL order (skipped if disabled)
```

Two orders coexist (documented, deliberate):

- **build execution order = topological order** (a plugin's `Deps` decide when
  its `build` runs, exactly like any `#[bean]`);
- **effect application order = install order** (`.plugin(A)` before
  `.plugin(B)` ⇒ A's layers/hooks apply before B's, regardless of which built
  first). Only observable for layer-order-sensitive plugin pairs.

### `Deps` — real topological edges, delivered to `build`

Any bean qualifies: `.provide()`-d values, factory-built beans
(`.register::<T>()`), beans from other plugins (e.g. Scheduler's
`Deps = (PoolExecutor,)` is an edge to the Executor plugin's projection).
Deps are appended to the builder's requirement list via
`RawPreStatePlugin::Required` and verified against the **final** provision
list at `build_state()` — nothing is checked at the `.plugin()` call site, so
a dependency may be supplied before *or after* the plugin is installed.
Missing → the standard guided "missing `.provide::<X>()` or `.register::<X>()`"
compile error.

There is **no shell/fill dance anymore**: a provided bean that depends on
another bean simply names it in `Deps` and builds directly from it inside
`build`. `Late<T>` survives only as an escape hatch for genuinely post-boot
fills (see below).

### Tuple `Provided` — group node + strict projections

`Provided` is always a tuple, mapped to the type-level provision list by
`PluginProvisions` (arities 0–8; a bare `type Provided = MyBean` gets an
on_unimplemented pointing to `(MyBean,)`). Mechanics: `.plugin()` registers
**one group node** (`PluginOut<Pl>`, holding the whole tuple; its runtime
dependencies come from `PluginDeps::dependencies()`) plus **one projection
node per element** that clones element *i* out of the group. Plugin nodes are
**volatile** — never reused across `r2e dev` hot-patch cycles.

**Duplicate/override semantics (breaking vs pre-W15, deliberate):**

- Projections register **strict**: an app `.provide()`/`.register()` of the
  same type, or installing the same plugin twice, is a `DuplicateBean` error
  at boot (previously a silent overwrite).
- **Pin-before-install wins**: `override_bean::<T>(mock)` before `.plugin()`
  makes the `T` projection an early-return — the graph holds the override.
  Pin-after-install is `DuplicateBean` (exact parity with `.register::<T>()`).
- **All-pinned skip is OPT-IN** (`const SKIP_BUILD_WHEN_ALL_PINNED: bool`,
  default `false`). By default `build` runs no matter how much is pinned:
  pinned projections still win element-wise, but effects (routes, layers,
  hooks, plugin data) are **not beans** and cannot be pinned, so "every
  `Provided` type is mocked" is no evidence the plugin is unwanted. Set the
  const to `true` only when `build` is pure bean construction *and* expensive.

| Plugin shape | `SKIP_BUILD_WHEN_ALL_PINNED` | Why |
|---|---|---|
| Build produces only its `Provided` beans, and costs I/O (connection, container, keygen) | `true` | Pinning them all really does replace the plugin — `OpenFga` (store/model resolution over gRPC) is the in-tree example |
| Build registers **any** effect (route, layer, `on_serve`, `on_shutdown`, `store_data`) | `false` (default) | OIDC provides one `Arc<JwtClaimsValidator>` and mounts `/oauth/token`, discovery, JWKS, `/userinfo` as effects — `true` would 404 them under any harness that pins the validator |
| Build is cheap | `false` (default) | Nothing to save |

To silence an effect-carrying plugin in a test, disable it
(`<prefix>.enabled = false`) — that is the switch that means "don't run".

### Typed `Config` — loaded before `build`, order-independent

- `type Config` must implement `PluginConfig`: implemented for `()` and
  blanket for any `ConfigProperties` — a `#[derive(ConfigProperties)]` struct
  is a valid `Config` as-is.
- Loaded **inside `build_state()`**, right before `build` runs — config is
  guaranteed loaded there, so `.plugin()` / `load_config()` order **does not
  matter** (the old "install Executor before load_config silently ignores
  `executor.*`" bug class is dead).
- Rules: `None` when `CONFIG_PREFIX` is `None`, no config was loaded, or no
  key lives under the prefix. A present-but-invalid section is a **boot
  error** naming the plugin (same `ConfigValidationError` report as a
  controller `#[config(section)]` mismatch).
- The section is parsed **even when the plugin is disabled** (`<prefix>.enabled
  = false`) — structural validation always happens; keep semantic
  (cross-field) validation behind your own `ctx.enabled()` check.
- Precedence convention: explicit builder setting (field on `self`) > file
  config > built-in default. Merge happens in `build` — which is why the
  plugin instance travels there by value.
- Raw access: `ctx.config_raw() -> Option<&R2eConfig>` for plugins without a
  typed section.

### `PluginBuildContext` — effects + graph access

Owned by the factory future (`'static`, no lifetime param):

| Method | Purpose |
|---|---|
| `enabled()` | `<prefix>.enabled` gate (default true) — check it, return a disabled variant |
| `graph() -> GraphHandle` | cheap cloneable **weak** handle on the **final** resolved graph (fills at the end of a successful `build_state()`; for request-time lookups, e.g. `Tenanted<T>`'s cascade) |
| `config_raw()` | the loaded `R2eConfig`, if any |
| `add_layer(f)` | router layer, plain closure (applied inside-out, install order) |
| `wrap_router(f)` | replace the whole router (e.g. gRPC multiplexer) — outside every layer |
| `store_data(d)` | type-keyed plugin data for post-state coordination (`app.get_plugin_data::<T>()`) |
| `on_serve(f)` | `FnOnce(ServeContext)` at serve time (spawn servers, start tasks) |
| `on_shutdown(f)` / `on_shutdown_async(f)` | graceful-shutdown hooks |
| `after_build(f)` | `FnOnce(&mut DeferredContext)` — full-graph boot-time escape hatch (replaces old `configure` residuals) |

All effects are buffered and applied after graph resolution, in install order.
When the plugin is disabled the **surface** effects are dropped and the
**cleanup** effects still run — see the split table under "Enabled gate" below.
Corollary: data that *other* subsystems read unconditionally (e.g. Scheduler's
`TaskRegistryHandle`, consumed by `#[scheduled]` collection even when the
scheduler is off) must be stored in `setup()` (ungated), not as a build effect.

**Resolve, don't capture (partial pins).** An effect closure that needs one of
the plugin's own provisions should read it from the graph at apply time —
`ctx.after_build(|dctx| { let x = dctx.bean_context().try_get::<X>()… })` —
rather than capturing the value `build` just made. A test that pins *some* of
the provisions (`override_bean(X::new())` without `SKIP_BUILD_WHEN_ALL_PINNED`
kicking in) still runs `build`: the plugin makes its own `X`, but the graph
exposes the pinned one, so a captured instance leaves the effect talking to an
object nobody can observe. Scheduler is the reference — its driver and handle
resolve `CancelToken` + `ScheduledJobRegistry` from `dctx` and fall back
to the built values only if the projection is absent. The exception is a
resource that is **not** a bean (the Scheduler's dedicated pool, the Executor's
pool as seen by its own drain): nothing else owns it, so capturing is correct.

### `setup()` — rare pre-graph escape hatch

Runs once at `.plugin()` time, before the graph (and possibly before config)
exists. Default no-op. Use it only for things other pre-state code must
observe: `store_data` that must exist even when disabled, `run_pre_destroy::<B>()`
lifecycle registrars, explicit low-level `add_deferred` actions.
`PluginSetupContext` = the old `PluginInstallContext` **minus**
`config()`/`config_get` (the "is config loaded yet?" trap is gone), minus
`run_post_construct` (obsolete: `build` is async — just await your init), and
minus **every surface effect**: there is no `add_layer`, no `wrap_router`, no
`on_serve`/`on_shutdown`/`on_shutdown_async` on the setup context.
Setup work is flushed as one deferred action, ordered
`[explicit add_deferred…, store_data flush, build effects]`.

**Why setup has no effects.** Setup actions are unconditional (below), so a
route or layer mounted from `setup` would survive `<prefix>.enabled = false` —
a disabled plugin serving traffic. Rather than gate the sugar (and re-open the
`TaskRegistryHandle` regression), the class was made unrepresentable: the
methods are gone, so `ctx.add_layer(...)` in `setup` is an E0599 compile error.
Surface belongs in `build`, where `ctx.enabled()` exists and the effect set is
dropped for you. `add_deferred` remains as the raw, explicitly **unconditional**
escape hatch — a `DeferredContext` in hand can mount anything, so it is the one
place where "disabled" is your own responsibility.

**Setup actions are never gated on `<prefix>.enabled`.** `setup()` is the
pre-graph coordination hook: what it deposits is what *other* pre-state code
reads, before and independently of the plugin doing any work. Gating it broke
exactly the promise the disabled Scheduler makes — with `scheduler.enabled =
false` the `TaskRegistryHandle` disappeared and every `#[scheduled]` /
`schedule_task` registration panicked with "Scheduler not installed" instead of
collecting tasks it never starts. If you want something in `setup()` to be
conditional, it does not belong in `setup()`: put it in `build`, where
`ctx.enabled()` is available and the whole effect set is dropped for you.

### Enabled gate: `<prefix>.enabled`

`.when()` cannot wrap `.plugin()` (type-level provision list is fixed), so
conditionality is runtime + config-driven. When `<prefix>.enabled = false`:

- `build` **still runs** (the `Provided` beans must exist — return a cheap,
  **inert** disabled variant after checking `ctx.enabled()`);
- **surface** effects are dropped, **cleanup** effects are not:

  | Registered via | Lane | Disabled plugin |
  |---|---|---|
  | `add_layer`, `wrap_router`, `on_serve`, `store_data`, `after_build` | surface | dropped |
  | `on_shutdown`, `on_shutdown_async` | cleanup | **still run** |

  Sync `on_shutdown` hooks are an **ordering** guarantee, not a liveness
  mechanism: they fire in registration order, one at a time (each is taken out
  of the shared cell before it runs and each runs under `catch_unwind`), so a
  panicking hook can neither discard the hooks queued behind it nor abort the
  shutdown sequence. What they do *not* cover is the exits that run no hooks at
  all — a panic unwinding out of `run_inner`, or the `run()` future being
  dropped under an `r2e dev` hot patch. Stopping background work must therefore
  hang off token *parentage*, not off a hook firing (see below).

  `build` ran, so it may have constructed something that must be released;
  dropping its disposal would leak exactly what a disabled plugin built (a
  disabled Executor's pool would never drain). Cleanup hooks run *before* the
  gate check, so they are registered whether or not anything else of the plugin
  is. Keep them to disposal — anything a disabled plugin should not do (start a
  driver, spawn a server, mount a route) belongs in the surface lane;
- setup-stored data and explicit `add_deferred` actions still run (see
  `setup()` above);
- the typed config section is still parsed (see Config above);
- "inert" is the plugin's own job: `ctx.enabled()` must be checked *before* any
  process-global side effect in `build` itself, not only around the effects —
  Prometheus, for instance, returns early so the global metrics recorder is
  never installed.

**The decision is taken once.** The group factory reads the gate from the
graph's `R2eConfig` — the same value `ctx.enabled()` hands to `build` — and
stores it in the effects slot alongside the effects it governs. The
install-order deferred action reads that carried flag; it does **not** re-read
`DeferredContext::config()` (the builder's own config), which can disagree with
the graph's when an `R2eConfig` bean is pinned. Recomputing it meant a plugin
could build itself live and have its routes dropped — or build an inert variant
whose routes were mounted anyway. The "plugin disabled" diagnostic is logged
from that same action, once per plugin.

Reference implementations: Prometheus (`prometheus.enabled: false` → no
`/metrics` route, no tracking layer, **no global recorder installed**, registry
bean still in graph), OpenFga (disabled variant fails every check closed with
`OpenFgaError::Disabled`), Tenancy (`TenantRouter::disabled`), Scheduler
(`scheduler.enabled: false` → no driver task, no handle extension;
`TaskRegistryHandle` still stored from `setup`, and the cancel-on-shutdown hook
still runs). Two plugins have **no** gate on purpose: `Executor` (`PoolExecutor`
is a compile-time dependency of `#[async_exec]`/`BackgroundService`/Scheduler,
so `executor.enabled = false` only logs a warning — bound it with
`executor.max-concurrent` instead) and `GrpcServer`/`OidcRuntime`/`OidcServer`
(`type Config = ()`, no `CONFIG_PREFIX`: they are never disabled).

### `GraphHandle`

```rust
#[derive(Clone, Default)]
pub struct GraphHandle(Late<Weak<BeanContext>>);
// fill(&Arc<BeanContext>) / get() -> Option<Arc<..>> / bean::<B>() -> Option<B>
```

Deferred-fill handle on the final resolved graph. This is `Late`'s remaining
first-party job; dogfood consumer: `TenantContext.beans` (per-tenant sources
resolve beans at request time).

- **The reference is weak.** The handle normally lives *inside* the graph it
  points at (`BeanContext → Tenanted<T> → GraphHandle`); a strong reference
  there is an unbreakable cycle — one leaked graph, with every pool and
  connection in it, per boot, i.e. per `r2e dev` hot-patch cycle.
- **The router owns the graph — and so does every request it started.**
  `build_inner` installs a pass-through `layers::GraphKeepAlive` layer
  **outermost** (after the transport wraps, so it also covers routes a plugin
  mounted through `add_layer`). Owning the `Arc` in the service value alone is
  *not* enough: `tower`'s `ServiceExt::oneshot` replaces the service with its
  future the moment `call` returns — before the first poll — and hyper splits
  the response into head and body, dropping the head (and any extension on it)
  while the body still streams. So the layer clones the `Arc` into the request
  **future**, and on completion moves it into the **response body**, releasing
  it when the last frame is produced. The body wrapper is a hand-written
  `GraphBody<B>` rather than `BodyExt::map_frame` for one concrete reason:
  `map_frame` does not forward `size_hint`, which strips `Content-Length` from
  **every** response in the framework and makes them all chunked (caught by
  `example-app`'s raw-socket test; pinned by
  `the_keep_alive_layer_preserves_the_body_size_hint`). The guarantee is
  therefore: *the graph outlives every request future and every response body
  derived from this router*, even when the router itself was already dropped.
- **Tracked work owns the graph while it runs.** The router dies with the serve
  future, which completes *before* tracked tasks (separate-port gRPC drain,
  `spawn_service` tasks, the scheduler driver, the live-config watch supervisor,
  the tenant sweeper, the QUIC endpoint drain) are awaited. Ownership is moved
  into the work itself rather than deduced from the exit path:
  `ServeContext::track` takes the **future**, not a `JobHandle`, and every
  tracked spawn goes through `ServiceHandles::spawn_owning`, which moves an
  `Arc<BeanContext>` into the task. `spawn_service`
  (`typed.rs::register_service`) and the QUIC drain use the same constructor.
  Pinned by `a_task_abandoned_by_the_grace_period_still_owns_its_graph` and
  `a_startup_error_cancels_and_drains_the_work_serve_hooks_started`.
  The complement is that `run()` never abandons tracked work silently: every
  exit it controls **cancels the shutdown token first, then joins the handles**
  — normal shutdown, a startup hook returning `Err` (serve hooks, which spawn
  the tasks, run first) and a serve error all go through the same
  cancel-then-drain (`prepared.rs::abort_started_work`). The abort path
  deliberately stops there: user `on_drain`/`on_stop` hooks and the async
  disposers do **not** run for a boot that never served.
  Two consequences worth knowing. (1) A tracked task that ignores the shutdown
  token holds up shutdown — the join is bounded only by `shutdown_grace_period`
  when one is set — and keeps its graph alive for as long as it runs; write
  tasks that observe the token. (2) Under `r2e dev` nothing joins anything: the
  patch drops the previous `run()` future, whose drop guard cancels that cycle's
  token, so its tracked tasks stop on their own while each keeps *its own*
  cycle's graph alive until it returns — the old graph is released when the last
  of them does. Serve hooks run only in the first cycle, so that work is not
  restarted afterwards.
- **Every framework-minted shutdown token descends from one root.** Owning the
  graph keeps tracked work *sound*; it still has to *stop*. The app shutdown
  token is created **lazily, at whichever comes first** — a
  `spawn_service`/`register_service` call or `run_inner` — and memoized in
  `plugin_data` (`ShutdownRoot`, `builder/mod.rs::shutdown_root`), instead of
  being created inside `run_inner` only, so
  `spawn_service` (`typed.rs::register_service`) can mint its per-service token
  as a `child_token()` of it before serving starts. Cancelling the root reaches
  every child, which is what covers the exits no hook survives: a panic
  unwinding out of `run_inner` (the cancel-on-drop guard fires during unwind)
  and a dropped `run()` future under `r2e dev`. The sync `on_shutdown` hook that
  cancels the same token in the normal sequence stays — it is what makes
  services stop *early* (before the HTTP drain), not what makes them stop at
  all. The scheduler's driver token is a plugin bean, not a framework child, so
  its tracked driver future relays the app token onto it explicitly
  (`r2e-scheduler/src/lib.rs`, `scheduled_tasks_driver`). Pinned by
  `dropping_the_run_future_stops_a_spawn_service_task` and
  `a_panicking_shutdown_hook_does_not_swallow_the_next_one`.
  **A plugin that mints its own `CancellationToken::new()` and relies solely on
  its sync `on_shutdown` hook to cancel it is stranded on those two paths** —
  either derive the token from something the framework cancels (a bean the
  scheduler-style relay reaches, or the `ServeContext::shutdown_token()` passed
  to `on_serve`) or accept that the task only stops on a clean shutdown.
- **The serve scope is the third owner.** `PreparedApp` holds its own strong
  `Arc` (`PreparedApp::graph`), moved into a `serve_scope_graph` local in
  `run_inner` and dropped only when that function returns. With tracked tasks
  now self-owning, this covers what is left: the shutdown phase itself
  (`on_stop` hooks and `#[pre_destroy]` disposers resolving through a
  `GraphHandle`) and in-flight WebSocket sessions.
  **Residual, WebSockets:** an upgraded connection is *not* part of graceful
  drain — hyper's `UpgradeableConnection::poll` hands the IO to the upgrade and
  returns `Ready(Ok(()))`, so the connection counts as finished, and axum's
  `on_upgrade` spawns a detached task with an empty 101 body. Sessions are
  covered for the whole of `run()` by the serve scope, but a session still alive
  after `run()` returns has no graph. In a binary the runtime drops immediately
  after, taking the task with it; an embedder that keeps the runtime alive must
  resolve what a session needs (pool, per-tenant resource) *before* the socket
  loop. Rejected fix: putting the `Arc` in request extensions so the generated
  `on_upgrade` closure could capture it — a boxed extension insert on every
  request to pay for a WS-only case.
- `get()`/`bean()` return `Some` throughout, `None` once the app and everything
  in flight are gone — treat that `None` as "we are shutting down", not as a
  bug. Whatever new entry point you add must keep the graph alive the same way
  or beans lose it mid-flight.
- **Filled on every *successful* exit of `try_build_state`** (cold, dev-reload,
  cached). A boot that fails returns through `?` before the fill, so a handle
  held from outside stays empty forever — there is no graph to point at.
- `fill` is public for hand-wired embedders (it takes `&Arc<..>` and downgrades;
  the caller keeps owning the `Arc`); `GraphHandle::default()` is an empty
  handle for tests.

## Plugins under `r2e dev` (hot patch cycles)

The group node and its per-provision projections are registered `volatile`:
their factories re-run on **every** cycle, fingerprint or not (a patched plugin
body must take effect, and the effects have to be re-registered on the fresh
router). That makes every plugin provision a *new instance* each cycle, so the
partial-rebuild pass seeds `forced_rebuild` with the volatile registrations:
every transitive dependent of a plugin bean rebuilds too. Without that, a reused
dependent would keep a clone of cycle N-1's provision while the graph exposed
cycle N's — e.g. a service holding a `Tenanted<T>` whose `GraphHandle` points at
the graph that was just dropped, failing every tenant lookup with `NoSource`.
The price is in-memory state loss for those dependents on each patch, dev-only,
and the same trade the decorator-target rule already makes.
Covered by `runtime/dev_reload.rs::a_rebuilt_plugin_bean_drags_its_dependents_with_it`
(needs `--features dev-reload`).

Known gaps (not fixed, see `docs/claude/roadmap.md`): startup lifecycle is
skipped once initialized, so a controller `#[post_construct]` never re-runs and
anything a patch *adds* (a new consumer, a new `#[scheduled]`) never starts; and
the dropped server future runs no shutdown hooks, so `#[pre_destroy]` /
plugin `on_shutdown*` do not fire between cycles. Cycle N-1's graph is not
dropped at the patch either: its drop guard cancels that cycle's token (which
reaches every derived token, so tracked tasks *do* stop), but each tracked task
holds the graph until it actually returns and nothing joins them — the old graph
is released when the last one exits. See the ownership rules above.

## Bean lifecycle hooks for `Provided` beans

- **PostConstruct**: no registrar anymore — `build` is async and fallible, so
  initialization happens inline (that was the whole point).
- **PreDestroy**: `ctx.run_pre_destroy::<B>()` from `setup` — runs during
  graceful shutdown in the async phase after the plugin's own
  `on_shutdown_async` hooks, reverse registration order, reading `B` from the
  resolved graph (so a pinned override is the value acted on).

## Module-declared required plugins

Unchanged by W15 (`Provisions = Provided::AsList` was preserved). A feature
module can declare `requires_plugins(Scheduler)` (macro) or
`type RequiredPlugins = (Scheduler,)` (hand-written `FeatureModule`); at
`register_module` the compiler checks every provided bean of each required
plugin is already in the provision list — i.e. the plugin was `.plugin(..)`-ed
before the module — with a plugin-named diagnostic
(`RequiredPluginInstalled` + `do_not_recommend`, `r2e-core/src/di/module.rs`).
Covered by `compile-fail/module_required_plugin_not_installed.rs`.

## RawPreStatePlugin (hidden escape hatch)

`#[doc(hidden)]`. HList-typed full-builder-access form that `.plugin()`
dispatches on; every `PreStatePlugin` gets it via the blanket impl
(`Provisions = Provided::AsList`, `Required = Deps::AsList`). The blanket impl
is where the mechanics live: setup flush (ungated), opt-in all-pinned skip, group + projection
registration, config load, enabled gate, effect drain, `BeanError::PluginBuild`
mapping. **Pitfall it guards against:** it calls
`crate::plugin::PreStatePlugin::build(plugin, ...)` fully qualified, because a
plugin with an inherent `build()` method (e.g. `OidcServer`'s builder-style
`fn build(self)`) would otherwise shadow the trait method. Implement
`RawPreStatePlugin` directly only to drive arbitrary builder methods during
install — no in-tree implementor remains.

## Testing plugins

- Unit: `AppBuilder::new().plugin(X).build_state().await`, assert beans via
  `state.get::<T>()` / `app.bean_context()`. Router surface: `.build()` on the
  typed app (NOT `with_state(())` — see above).
- Mocking: `override_bean` each `Provided` type **before** `.plugin()` — the
  pins win, and `build` still runs (with its effects) unless the plugin sets
  `SKIP_BUILD_WHEN_ALL_PINNED = true`; use `<prefix>.enabled=false` for an
  inert install.
- Config: `with_config` an in-memory `R2eConfig`; order-independence and
  validation-panic cases in `r2e-core/tests/plugin/config.rs`.
- Boot failure: assert on `try_build_state().await` `Err` or
  `#[should_panic(expected = "Plugin '...' failed to build")]`.
- The core suite (`r2e-core/tests/plugin/`) is organized as: `deps.rs`,
  `config.rs`, `enabled.rs`, `lifecycle.rs`, `provisions.rs`, `deferred.rs`,
  `setup.rs`, `late.rs`.
