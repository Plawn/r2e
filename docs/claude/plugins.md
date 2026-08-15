# Plugin System Reference

Authoritative reference for R2E's plugin system as of the **factory-first
redesign (W15, 2026-08-15)**: `PreStatePlugin` is one async, fallible factory
(`build`) executed inside `build_state()` as a node of the bean graph. The old
two-phase `install`/`configure` machine — and the `Late<T>` shell-then-fill
dance it forced — is gone. Source of truth: `r2e-core/src/plugin.rs`,
`r2e-core/src/type_list.rs` (`PluginDeps` / `PluginProvisions`),
`r2e-core/src/config/mod.rs` (`PluginConfig`), `r2e-core/src/late.rs`
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
bypasses the graph entirely, so plugin-provided routes/layers never appear
there. Tests exercising a plugin's router surface must go through
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
- **All-pinned skip**: if a test pins *every* `Provided` type, `build` is
  skipped entirely (no side effects). Pinning only *some* still runs `build`
  (the group yields the whole tuple); to also silence side effects, set
  `<prefix>.enabled = false`.

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
| `graph() -> GraphHandle` | cheap cloneable handle on the **final** resolved graph (fills at the end of `build_state()`; for request-time lookups, e.g. `Tenanted<T>`'s cascade) |
| `config_raw()` | the loaded `R2eConfig`, if any |
| `add_layer(f)` | router layer, plain closure (applied inside-out, install order) |
| `wrap_router(f)` | replace the whole router (e.g. gRPC multiplexer) — outside every layer |
| `store_data(d)` | type-keyed plugin data for post-state coordination (`app.get_plugin_data::<T>()`) |
| `on_serve(f)` | `FnOnce(ServeContext)` at serve time (spawn servers, start tasks) |
| `on_shutdown(f)` / `on_shutdown_async(f)` | graceful-shutdown hooks |
| `after_build(f)` | `FnOnce(&mut DeferredContext)` — full-graph boot-time escape hatch (replaces old `configure` residuals) |

All effects are buffered and applied after graph resolution, in install order —
and **dropped wholesale when the plugin is disabled**. Corollary: data that
*other* subsystems read unconditionally (e.g. Scheduler's `TaskRegistryHandle`,
consumed by `#[scheduled]` collection even when the scheduler is off) must be
stored in `setup()` (ungated), not as a build effect.

### `setup()` — rare pre-graph escape hatch

Runs once at `.plugin()` time, before the graph (and possibly before config)
exists. Default no-op. Use it only for things other pre-state code must
observe: `store_data` that must exist even when disabled, `run_pre_destroy::<B>()`
lifecycle registrars, explicit low-level `add_deferred` actions.
`PluginSetupContext` = the old `PluginInstallContext` **minus**
`config()`/`config_get` (the "is config loaded yet?" trap is gone) and minus
`run_post_construct` (obsolete: `build` is async — just await your init).
Setup-time sugar is flushed as one enabled-gated deferred action, like before.

### Enabled gate: `<prefix>.enabled`

`.when()` cannot wrap `.plugin()` (type-level provision list is fixed), so
conditionality is runtime + config-driven. When `<prefix>.enabled = false`:

- `build` **still runs** (the `Provided` beans must exist — return a cheap
  disabled variant after checking `ctx.enabled()`);
- all effects registered on the build context are **dropped**;
- setup-time sugar and explicit `add_deferred` actions are skipped;
- the typed config section is still parsed (see Config above).

Reference implementations: Prometheus (`prometheus.enabled: false` → no
`/metrics` route, no tracking layer, registry bean still in graph), OpenFga
(disabled variant fails every check closed with `OpenFgaError::Disabled`),
Scheduler (`scheduler.enabled: false` → no driver task; `TaskRegistryHandle`
still stored, from `setup`).

### `GraphHandle`

```rust
#[derive(Clone, Default)]
pub struct GraphHandle(Late<Arc<BeanContext>>);  // fill/get/bean::<B>()
```

Deferred-fill handle on the final resolved graph, filled by the builder on
every `try_build_state` exit path (cold, dev-reload, cached). `fill` is public
for hand-wired embedders; `GraphHandle::default()` is an empty handle for
tests. This is `Late`'s remaining first-party job; dogfood consumer:
`TenantContext.beans` (per-tenant sources resolve beans at request time).

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
(`RequiredPluginInstalled` + `do_not_recommend`, `r2e-core/src/module.rs`).
Covered by `compile-fail/module_required_plugin_not_installed.rs`.

## RawPreStatePlugin (hidden escape hatch)

`#[doc(hidden)]`. HList-typed full-builder-access form that `.plugin()`
dispatches on; every `PreStatePlugin` gets it via the blanket impl
(`Provisions = Provided::AsList`, `Required = Deps::AsList`). The blanket impl
is where the mechanics live: setup flush, all-pinned skip, group + projection
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
- Mocking: `override_bean` each `Provided` type **before** `.plugin()`
  (all-pinned ⇒ build skipped); or `<prefix>.enabled=false` for an inert
  install.
- Config: `with_config` an in-memory `R2eConfig`; order-independence and
  validation-panic cases in `r2e-core/tests/plugin/config.rs`.
- Boot failure: assert on `try_build_state().await` `Err` or
  `#[should_panic(expected = "Plugin '...' failed to build")]`.
- The core suite (`r2e-core/tests/plugin/`) is organized as: `deps.rs`,
  `config.rs`, `enabled.rs`, `lifecycle.rs`, `provisions.rs`, `deferred.rs`,
  `late.rs`.
