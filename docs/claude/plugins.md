# Plugin System Reference

Authoritative reference for R2E's plugin system as of the **one-plugin-kind
unification (Plan 1, 2026-08-26)**, which sits on top of the **factory-first
redesign (W15, 2026-08-15)**. There is exactly **one** plugin trait, one
install site, and one factory method:

- `Plugin` (formerly `PreStatePlugin`) — installed with `.plugin(p)`, always
  **before** `build_state()`; its `build` runs inside `build_state()` as a node
  of the bean graph.
- The post-state `Plugin` trait, its `.with(p)` install call and the advisory
  `should_be_last()` are **removed**. `RawPreStatePlugin` is now
  `PluginInstall`.

Source of truth: `r2e-core/src/plugin/` (`pre_state.rs` = the trait +
`PluginInstall` blanket impl, `contexts.rs` = the three effect contexts,
`graph_handle.rs`, `deferred.rs`), `r2e-core/src/type_list.rs` (`PluginDeps` /
`PluginProvisions`), `r2e-core/src/di/module.rs` (plugin controllers),
`r2e-core/src/config/mod.rs` (`PluginConfig`), `r2e-core/src/builtins/`
(the built-ins), `r2e-core/src/di/late.rs` (`Late<T>`, escape hatch only).

## One plugin kind

| | |
|---|---|
| Trait | `Plugin` |
| Install call | `.plugin(p)`, **before** `build_state()` |
| Provides beans | yes — tuple `Provided` |
| Ships controllers | yes — tuple `Controllers` |
| Typed config | yes — `Config` + `CONFIG_PREFIX` |
| Effects | three stages: Graph → Routes → Finalize |
| Escape hatch | `PluginInstall` (`#[doc(hidden)]`, blanket impl over `Plugin`) |

`Health`, `Cors`, `Tracing`, `ErrorHandling`, `NormalizePath`, `DevReload`,
`SecureHeaders`, `RequestIdPlugin`, `OpenApiPlugin`, `EmbeddedFrontend` are
ordinary `.plugin()` calls now, exactly like `Scheduler` or `Prometheus`.
Nothing needs to be installed last: what used to require "install me after
every controller" registers a **Routes**-stage effect, and what used to require
"install me outermost" registers a **Finalize**-stage effect.

### Migration from the old two-kind model

| Before (post-state) | Now |
|---|---|
| `.build_state().await.with(Health)` | `.plugin(Health).build_state().await` |
| `.with(Health::builder().check(..).build())` | `.plugin(Health::builder().check(..).build())` |
| `.with(Cors::permissive())` | `.plugin(Cors::permissive())` |
| `.with(Tracing)` / `.with(Tracing::configured(cfg))` | `.plugin(Tracing)` / `.plugin(Tracing::configured(cfg))` |
| `.with(ErrorHandling)` | `.plugin(ErrorHandling)` |
| `.with(NormalizePath)` | `.plugin(NormalizePath)` |
| `.with(DevReload)` | `.plugin(DevReload)` |
| `.with(SecureHeaders::default())` | `.plugin(SecureHeaders::default())` |
| `.with(RequestIdPlugin)` | `.plugin(RequestIdPlugin)` |
| `.with(OpenApiPlugin::new(cfg))` **last** | `.plugin(OpenApiPlugin::new(cfg))` anywhere |
| `.with(EmbeddedFrontend::new::<Assets>())` | `.plugin(EmbeddedFrontend::new::<Assets>())` |
| `impl Plugin for X { fn install(self, b) -> AppBuilder … }` | `impl Plugin for X { type Provided/Deps/Config/Controllers; async fn build(..) }` |
| `fn should_be_last() -> bool { true }` | delete it — use `after_routes` / `wrap_router` |

Order of installs no longer differs from order of `build_state()`: every
`.plugin()` goes on the *typed* builder, so the whole app is one chain ending
in `build_state().await`.

**Plugins only run on the graph path.** `build_state().await.build()` (or
`serve*`) executes plugin builds; the legacy `with_state(())` shortcut throws
the bean registry away, so the group node never runs: no `build`, no
provisions, no effects (any stage), no serve/cleanup hooks. What *does* still
run there is `setup()` and the deferred actions it queued (`store_data`,
`add_deferred`) — they are pre-graph by construction. The plugin's effect
action then finds its slot empty and logs a `debug!` no-op;
`.plugin(P).with_state(())` is a supported (if pointless) combination, not a
panic. Tests exercising a plugin's router surface must go through
`build_state()`.

## Effect stages

A plugin never touches the router directly; it registers **effects** on the
`PluginBuildContext`, and each effect belongs to one of three stages:

| Stage | Registered with | Applied |
|---|---|---|
| **Graph** | `add_layer`, `after_build`, `on_serve`, `on_serve_each_cycle`, `store_data` | inside `build_state()`, right after the graph resolves |
| **Routes** | `after_routes(FnOnce(&mut RoutesContext))` | in `build()`, after **every** controller (app, module, plugin) is registered |
| **Finalize** | `wrap_router(FnOnce(Router) -> Router)` | in `build()`, outermost — after every HTTP layer |

Two orders coexist (documented, deliberate):

- **build execution order = topological order** (a plugin's `Deps` decide when
  its `build` runs, exactly like any `#[bean]`);
- **effect application order = install order**, *within a stage*: `.plugin(A)`
  before `.plugin(B)` ⇒ A's Graph effects apply before B's, A's Routes effects
  before B's, A's Finalize wrap before B's — regardless of which built first.
  For layers and wraps "before" means **inner**: the later install ends up
  outside the earlier one.

The concrete assembly order inside `build()` (`typed.rs::build_inner`) is:

```
controller routes (app + module + plugin)
  → meta consumers
  → .with_state(state)
  → ROUTES effects            (routers merged here, so they get the layers below)
  → Graph `add_layer`s        (install order, later = outer)
  → NormalizePath (if any)
  → catch_panic_layer         (always)
  → FINALIZE `wrap_router`s   (install order, later = outer)
  → graph keep-alive          (outermost, framework-owned)
```

Which stage do I want?

- **Graph** — a middleware layer, plugin data, a serve-time task, a
  full-graph escape hatch. The default.
- **Routes** — anything that must see the *complete* route table: OpenAPI spec
  generation, a route dump, a router mounted from route metadata. `after_routes`
  hands you a `RoutesContext`; see below.
- **Finalize** — a transport-level wrap that must sit outside every HTTP layer:
  a gRPC/HTTP multiplexer, a protocol switch. A JSON 500 from `catch_panic` is
  garbage to a gRPC client, hence "outside everything HTTP-shaped".

A plugin disabled with `<prefix>.enabled = false` drops **all three** stages;
its cleanup hooks (`on_shutdown` / `on_shutdown_async`) still run, because
`build` — and whatever it constructed — ran anyway. See "Enabled gate" below.

### `RoutesContext` — the route registry

```rust
ctx.after_routes(move |routes: &mut RoutesContext| {
    let all: &[RouteInfo] = routes.routes();       // every registered route
    routes.register_routes(some_router);           // mount your own
    let cfg = routes.config();                     // Option<&R2eConfig>
    let beans = routes.bean_context();             // resolved graph
    let d = routes.take_data::<MyData>();          // plugin data from setup/Graph
});
```

`routes()` is the **route registry**: the `RouteInfo` list collected by
`register_controller` plus the module and plugin controller folds (it is the
`MetaRegistry`'s `RouteInfo` collection — see the deviation note in
`plans/plugin-module-lifecycle-unification.md`). Because the Routes stage runs
after *all* controllers are registered, a Routes effect is install-order
independent. `OpenApiPlugin` is the reference implementation: it builds the
spec from `routes.routes()` and mounts `/openapi.json` (+ `/docs`) with
`routes.register_routes(..)`, which is why it no longer has to be installed
last.

## `Plugin` surface

```rust
impl Plugin for MyPlugin {
    type Provided    = (MyService,);          // tuple: (A,), (A, B), or () — never a bare type
    type Deps        = (DbPool, PoolExecutor); // real topo edges; arrive built, by value
    type Config      = MyConfig;              // or (); #[derive(ConfigProperties)] section
    type Controllers = (MyController,);       // or (); #[controller] types this plugin ships
    const CONFIG_PREFIX: Option<&'static str> = Some("my-plugin");
    // const BUILD_VERSION: u64 = 0;          // optional dev-reload stamp, rarely needed

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

All four associated types are mandatory (stable Rust has no associated-type
defaults): write `type Controllers = ();` when the plugin ships none.

`PluginBuildError = Box<dyn std::error::Error + Send + Sync>`. An `Err` from
`build` aborts startup as `BeanError::PluginBuild { plugin, source }`
(Display: ``Plugin '<name>' failed to build: <source>``); `build_state()`
panics with `Failed to resolve bean dependency graph: <that message>`, so
`#[should_panic(expected = "...")]` substring tests work.

### `Controllers` — plugins ship endpoints the normal way

```rust
#[controller(path = "/metrics")]
pub struct MetricsController {
    #[inject] registry: MetricsRegistry,     // the plugin's own Provided bean
    #[inject] clock: AppClock,               // …or any app bean
}

impl Plugin for Metrics {
    type Provided    = (MetricsRegistry,);
    type Controllers = (MetricsController,);
    // …
}
```

Mechanics: `.plugin()` folds `Controllers` into the same pending-controller
list (`Mods`) feature modules use, so plugin controllers are registered by
`build_state()` right after the graph resolves — before the Routes stage, so
their routes are in `routes.routes()`. Their `#[inject]` fields are checked
against the **final** provision list at `build_state()` exactly like `Deps`:
a missing bean is a compile error naming the plugin
(`PluginControllerList` + `#[diagnostic::on_unimplemented]`, covered by
`compile-fail/plugin_controller_dep_missing.rs`).

This is the preferred way for a plugin to expose HTTP: guards, `#[roles]`,
OpenAPI metadata and request extraction all work as usual, instead of
hand-assembling a `Router` inside `build`.

### Lifecycle

```
.plugin(Me)                  build_state()                     build()            (serve)
     │                            │                               │                  │
     ▼                            ▼                               ▼                  ▼
 setup(&mut self)     graph resolution (topological):        Routes effects      on_serve hooks
   registry node        deps built → build(deps, cfg, ctx)    then Finalize
   + controllers      then Graph effects, install order        wraps
     queued           then plugin controllers registered
```
### `Deps` — real topological edges, delivered to `build`

Any bean qualifies: `.provide()`-d values, factory-built beans
(`.register::<T>()`), beans from other plugins (e.g. Scheduler's
`Deps = (PoolExecutor,)` is an edge to the Executor plugin's projection).
Deps are appended to the builder's requirement list via
`PluginInstall::Required` and verified against the **final** provision
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
  same type is a `DuplicateBean` error at boot (previously a silent
  overwrite). Installing the **same plugin twice** (app + module, or two
  modules) is the dedicated `BeanError::DuplicatePlugin`, which names both
  owners and points at `requires_plugins` — see
  [Modules and plugins](#modules-and-plugins).
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

| Method | Stage | Purpose |
|---|---|---|
| `enabled()` | — | `<prefix>.enabled` gate (default true) — check it, return a disabled variant |
| `graph() -> GraphHandle` | — | cheap cloneable **weak** handle on the **final** resolved graph (fills at the end of a successful `build_state()`; for request-time lookups, e.g. `Tenanted<T>`'s cascade) |
| `config_raw()` | — | the loaded `R2eConfig`, if any |
| `add_layer(f)` | Graph | router layer, plain closure (applied inside-out, install order) |
| `store_data(d)` | Graph | type-keyed plugin data for cross-plugin coordination (`app.get_plugin_data::<T>()`, `RoutesContext::take_data`) |
| `on_serve(f)` | Graph | `FnOnce(ServeContext)` at serve time (spawn servers, start tasks) |
| `on_serve_each_cycle(f)` | Graph | same, but **also runs on `r2e dev` hot-patch cycles** (plain `on_serve` is skipped there). For transports that own a port: a patch drops the previous `run()`, which cancels that cycle's shutdown token (tracked tasks stop cooperatively — they are detached, not aborted), so the port must be re-served each cycle — bind via `ServeContext::bind_tcp(owner, addr)` (async; dev listener store keyed by `(owner, addr)`: same socket across cycles, no sharing with HTTP) and serve through `BoundListener::into_incoming(shutdown)`, whose stream stops before any accept once shutdown or the next cycle's handover fires and then releases the socket; the next cycle's `bind_tcp` waits for that release (5 s bound, then warns and proceeds). Must be safe to re-run. |
| `after_build(f)` | Graph | `FnOnce(&mut DeferredContext)` — full-graph boot-time escape hatch |
| `after_routes(f)` | Routes | `FnOnce(&mut RoutesContext)` — runs after every controller is registered: read the route registry, mount routers from it |
| `wrap_router(f)` | Finalize | replace the whole router (e.g. gRPC multiplexer) — outside every HTTP layer, `catch_panic` included |
| `on_shutdown(f)` / `on_shutdown_async(f)` | cleanup | graceful-shutdown hooks (never gated on `enabled`) |

All effects are buffered; Graph effects are applied after graph resolution
inside `build_state()`, Routes and Finalize effects inside `build()` — each
stage in install order.
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
  | `add_layer`, `store_data`, `on_serve`, `after_build` | surface (Graph) | dropped |
  | `after_routes` | surface (Routes) | dropped |
  | `wrap_router` | surface (Finalize) | dropped |
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
  token holds up shutdown — the join is bounded by `shutdown_grace_period` when
  one is set, applied **per handle**, after which that task is abandoned with a
  `warn!` naming it (use `track_named` so the name is useful) — and keeps its
  graph alive for as long as it runs; write tasks that observe the token. (2) Under `r2e dev` nothing joins anything: the
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
Covered by `dev_reload/cycles.rs::a_rebuilt_plugin_bean_drags_its_dependents_with_it`
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

## Modules and plugins

A feature module relates to a plugin in exactly one of two ways.

| | Macro key | `FeatureModule` item | Meaning |
|---|---|---|---|
| **Bring** | `plugins(Scheduler = Scheduler)` | `type Plugins = (Scheduler,)` + `fn plugins()` | the module **installs** the plugin, as if `.plugin(Scheduler)` sat at the `register_module` call site |
| **Require** | `requires_plugins(Scheduler)` | `type RequiredPlugins = (Scheduler,)` | the module only **needs** the plugin installed — by the app or by another module |

### Bring — `plugins(Type = expr, ...)`

The `Type = expr` form is mandatory (a bare type or a missing `=` is a
targeted macro error): the **type** is needed at compile time — it grows the
provision list — while the **expression** is the instance to install, so a
plugin that needs builder configuration still works
(`plugins(Executor = Executor::with_max_concurrent(8))`).

At `register_module` the brought plugins are installed **first**, before the
module's own providers are registered and scope-checked, by folding
`M::plugins()` through the same `.plugin()` machinery
(`ModulePlugins<P, R, Mods>`, `r2e-core/src/di/module.rs`). Consequences:

- their `Provisions` join the **app-global** `P` and their `Deps` join `R`,
  exactly like an app-level `.plugin(..)` at that position;
- their `Controllers` are queued through the usual `PushPluginCtrls` fold, so
  a plugin's endpoints appear whether the app or a module installed it;
- their effects land at this `register_module` call's position in **install
  order** — a plugin brought by the second of two modules applies its Graph
  layer / Routes effect / Finalize wrap after everything installed before that
  `register_module` call, and before anything installed after it (see
  [Effect stages](#effect-stages));
- the module's own providers and controllers may depend on the brought
  plugin's beans (they are part of the module scope), and so may the rest of
  the app — a brought plugin's bean is app-global, needs **no** `exports(..)`
  entry, and **must not** appear in one (`Exports` is still checked against
  the module's own `Providers`; listing a plugin bean there would put the same
  type in two state slots and break `HasBean` index inference).

### Require — `requires_plugins(Type, ...)`

Unchanged by W15 (`Provisions = Provided::AsList` was preserved). At
`register_module` the compiler checks every provided bean of each required
plugin is present in the provision list **after** the module's own brought
plugins were folded in — so a plugin may be satisfied by the app, by a module
registered earlier, or by this module's own `plugins(..)`. The diagnostic is
plugin-named (`RequiredPluginInstalled` + `do_not_recommend`). Covered by
`compile-fail/module_required_plugin_not_installed.rs`.

### Ownership rule — exactly one owner (decided 2026-08-26)

A plugin is installed by exactly one owner: the app **or** one module. Every
other module that merely needs it uses `requires_plugins`. A double install is
a boot error — `BeanError::DuplicatePlugin`, which records the owner label per
plugin group node at registration and renders both:

```text
Plugin 'BroughtPlugin' is installed by app and by module 'BillingModule'.
A plugin has exactly one owner — the app or ONE module. Use
`requires_plugins(BroughtPlugin)` in every module that only needs it, and keep
the single `.plugin(BroughtPlugin)` / `plugins(BroughtPlugin = ..)` install.
```

(Mechanically: `BeanRegistry::set_plugin_owner` brackets the module's plugin
fold, `register_plugin_group` records the owner for `PluginOut<Pl>`, and
`check_for_duplicates` upgrades the generic `DuplicateBean` to
`DuplicatePlugin` when the duplicated node is a plugin group with more than
one recorded owner. The group node is registered before its projections, so
the specific message always wins over an opaque projection duplicate.)

Hand-written `FeatureModule` impls must supply both items — stable Rust has no
associated-type defaults, so `Plugins` is a required associated type; a module
that brings nothing writes:

```rust
type Plugins = ();
fn plugins() {}
```

## `PluginInstall` (hidden escape hatch)

`#[doc(hidden)]`. HList-typed full-builder-access form that `.plugin()`
dispatches on; every `Plugin` gets it via the blanket impl
(`Provisions = Provided::AsList`, `Required = Deps::AsList`). The blanket impl
is where the mechanics live: setup flush (ungated), opt-in all-pinned skip, controller fold, group + projection
registration, config load, enabled gate, effect drain, `BeanError::PluginBuild`
mapping. **Pitfall it guards against:** it calls
`crate::plugin::Plugin::build(plugin, ...)` fully qualified, because a
plugin with an inherent `build()` method (e.g. `OidcServer`'s builder-style
`fn build(self)`) would otherwise shadow the trait method. Implement
`PluginInstall` directly only to drive arbitrary builder methods during
install — no in-tree implementor remains.

## `HealthRegistry` — plugins contributing health checks

Two health plugins, one difference: what they provide.

| Plugin | Install | `Provided` | Routes (Routes stage) |
|---|---|---|---|
| `Health` | `.plugin(Health)` | `()` | `GET /health` → `200 "OK"` |
| `AdvancedHealth` | `.plugin(Health::builder().check(..).build())` | `(HealthRegistry,)` | `GET /health`, `/health/live`, `/health/ready` |

`HealthRegistry` is the extension point: any plugin can declare
`type Deps = (HealthRegistry,)` and register an indicator from its `build`,
so a datasource, a broker client or a cache contributes its own readiness
probe without the app wiring anything:

```rust
impl Plugin for MyBackend {
    type Provided = (MyClient,);
    type Deps = (HealthRegistry,);
    type Config = ();
    type Controllers = ();

    async fn build(self, (health,): Self::Deps, _c: Option<()>, _ctx: &mut PluginBuildContext)
        -> Result<Self::Provided, PluginBuildError> {
        let client = MyClient::connect(self.url).await?;
        health.register(MyClientHealth::new(client.clone()));   // HealthIndicator
        Ok((client,))
    }
}
```

Because `HealthRegistry` is a bean and `Deps` are real topological edges, the
contributor builds *after* `AdvancedHealth` whatever the install order — and
forgetting `.plugin(Health::builder()…)` is the standard missing-bean compile
error, not a silent no-op. `HealthIndicator` implementors report
`HealthStatus::Up/Down` plus optional details; `HealthRegistry::set_cache_ttl`
throttles expensive probes.

In-tree contributor: `DataSourceHealth<DB, Tag>` in `r2e-data-sqlx` /
`r2e-data-diesel` (`Deps = (DbPool<DB, Tag>, HealthRegistry)`), which runs a
`SELECT 1` and names its check after the datasource tag (`db`, `db:reporting`).

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
- Stages: `r2e-core/tests/plugin/stages.rs` pins stage order
  (Graph → Routes → Finalize), install order within a stage, the disabled-plugin
  semantics, and that `after_routes` sees a controller registered *after* the
  plugin. Layer order across plugins (e.g. `ErrorHandling` first, an observer
  layer after it seeing the 500) is pinned in `r2e-core/tests/http/plugins.rs`.
- Plugin controllers: `r2e-core/tests/plugin/controllers.rs` (injecting the
  plugin's own `Provided` + app beans);
  `r2e-compile-tests/cases/plugins/fail/plugin_controller_dep_missing.rs` pins
  the plugin-named compile error.
- Health contributions: `r2e-core/tests/plugin/health_registry.rs` (several
  plugins registering into one `HealthRegistry`).
- The core suite (`r2e-core/tests/plugin/`) is organized as: `deps.rs`,
  `config.rs`, `enabled.rs`, `lifecycle.rs`, `provisions.rs`, `deferred.rs`,
  `setup.rs`, `late.rs`, `stages.rs`, `controllers.rs`, `health_registry.rs`.
