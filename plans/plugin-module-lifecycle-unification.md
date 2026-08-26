# Plugin / module / lifecycle unification (Quarkus-extension parity)

Status: **proposal**, 2026-08-26. Not scheduled. R2E is not in production, so
every phase below is allowed to break the public API; breaking changes are
listed per plan.

## Why

Comparing R2E with Quarkus (extensions + SmallRye + CDI lifecycle) exposed
three orthogonal axes that Quarkus collapses into one *extension*:

| Axis | R2E today | Quarkus |
|---|---|---|
| dependency | Cargo feature on the `r2e` façade | Maven artifact `quarkus-xxx` |
| infrastructure contributor (beans, layers, routes, lifecycle) | `PreStatePlugin` (`.plugin()`, before `build_state()`) **and** `Plugin` (`.with()`, after) | the extension itself |
| DI scoping unit | `#[module]` (NestJS lineage, compile-time encapsulation) | — (CDI has no encapsulation) |

The mapping is documented elsewhere (`docs/claude/plugins.md`,
`docs/claude/di-builder-refactor.md`). What is *not* Quarkus-grade:

1. The pre-state / post-state split is an implementation leak. A plugin
   cannot register R2E controllers (only raw `Router` layers); the real
   boundary is "needs the final route set" (OpenAPI), not "provides beans".
2. A module can *require* a plugin (`requires_plugins`) but cannot *bring*
   it, so a module is not a self-contained unit of reuse.
3. Beans cannot observe startup (`StartupEvent`); `on_start` is a builder
   closure only.
4. There is no datasource plugin: apps `.provide(pool)` and run migrations
   in `on_start` (`examples/example-postgres/src/app.rs:47`).

Plans 1–3 address the three gaps; plan 4 is the datasource plugin that
motivates them and doubles as the acceptance test.

Current anchors (verify before coding — they move):

- `r2e-core/src/plugin/pre_state.rs` — `PreStatePlugin`, blanket `RawPreStatePlugin`
- `r2e-core/src/plugin/post_state.rs` — `Plugin` (`install(self, AppBuilder<T>)`)
- `r2e-core/src/plugin/contexts.rs` — `PluginSetupContext`, `PluginBuildContext`
  (`add_layer`, `wrap_router`, `store_data`, `on_serve`, `on_shutdown*`, `after_build`)
- `r2e-core/src/plugin/deferred.rs` — `DeferredContext`
- `r2e-core/src/builtins/mod.rs` — post-state builtins: `Health`, `AdvancedHealth`,
  `Cors`, `Tracing`, `ConfiguredTracing`, `ErrorHandling`, `DevReload`; plus
  `builtins/request_id.rs`, `builtins/secure_headers.rs`; `r2e-openapi/src/ext.rs`
- `r2e-core/src/di/module.rs` — `FeatureModule`, `ModuleControllers`, `ModuleList`,
  `RequiredPluginsInstalled`
- `r2e-core/src/builder/nostate.rs` — `plugin()`, `register_module_impl`, `build_state`
- `r2e-core/src/builder/typed.rs` — `with()`, `on_start`/`on_stop`/`on_drain`,
  `register_routes`, `build`, `serve*`, `#[pre_destroy]` assembly (~L612, ~L849)
- `r2e-macros/src/codegen/transverse.rs` — shared bean/controller-core codegen
  (`#[post_construct]`, `#[pre_destroy]` live here)

---

## Plan 1 — One plugin kind

### Goal

A single `Plugin` trait (today's `PreStatePlugin`, renamed), installed with
`.plugin()` before `build_state()`. It can provide beans, layers, raw routes,
**R2E controllers**, lifecycle hooks, and — for the OpenAPI case — effects
that run once the final route set exists. `Plugin` (post-state) and `.with()`
are removed.

### Design

**1a. Effect stages.** `PluginBuildContext` effects get an explicit stage
instead of "install order on one list":

```rust
enum EffectStage {
    Graph,       // today: right after graph resolution, install order
    Routes,      // after ALL controllers (app + modules + plugins) are registered
    Finalize,    // after Routes; the router is complete (OpenAPI, NormalizePath)
}
```

- `add_layer` stays `Graph` (inner layers, install order — unchanged
  semantics).
- `wrap_router` stays outermost and moves to `Finalize` (it already means
  "transport-level, applied last"; `should_be_last()` becomes redundant).
- New: `ctx.after_routes(FnOnce(&mut RoutesContext))` runs at `Routes`.
  `RoutesContext` exposes the collected route metadata (see 1c) and
  `register_routes(Router<()>)` for state-less routers.
- Within a stage, order = install order (documented, unchanged rule).

Implementation: `EffectSet { surface, routes, finalize, shutdown }` in
`contexts.rs`; `build_state()` drains `Graph` where it drains `surface` today;
the typed builder's `build()` drains `Routes` then `Finalize` after the
`Mods` fold and after all `register_controller` calls — i.e. at the point
where the router is about to be assembled (`typed.rs::build`, ~L753).

**1b. Plugin controllers.** Add to the trait:

```rust
type Controllers: PluginControllers = ();   // tuple of #[controller] types
```

Reuse the module machinery: `register_module_impl` already queues a deferred
controller fold on `Mods`; `plugin()` pushes an equivalent entry
(`PluginCtrls<Pl>`) onto the same `Mods` list, so `build_state()` folds plugin
controllers with `ModuleList::register_controllers`, in install order, with
the same `EndpointDeps` / `AllSatisfied` compile check against the final `P`.
Plugin controllers may `#[inject]` the plugin's own `Provided` beans and any
bean in `P`. This is what makes a datasource plugin able to ship an admin
controller, or Prometheus/OIDC able to drop their hand-rolled axum routers.

No new trait: `PluginControllers` is `ModuleControllers` re-exported under a
plugin-facing name (or simply `type Controllers` bound to
`ModuleControllers<T, W>` through the fold).

**1c. Route metadata as data, not ordering.** OpenAPI today needs to be
`.with()`-ed after every `register_controller`. Replace with a
`RouteRegistry` deposited in the builder by `register_controller` (already
collects `RouteMeta` for OpenAPI — check `r2e-openapi/src/ext.rs` for what it
reads) and handed to `Routes`-stage effects via `RoutesContext::routes()`.
`OpenApiPlugin` becomes a `Plugin` with `Provided = ()`, `after_routes` builds
the spec and registers `/openapi.json` + `/docs`.

**1d. Health becomes a registry bean.** `AdvancedHealth` →
`Provided = (HealthRegistry,)`; `HealthRegistry` is `Clone` + interior
mutability, checks are pushed by other plugins from `build`
(`Deps = (HealthRegistry,)`). That's Quarkus' "datasource contributes a
readiness check" (used by plan 4). `Health` (simple) keeps `Provided = ()`.

**1e. Migrate builtins.** Every post-state plugin becomes a `Plugin`:

| plugin | `Provided` | effect |
|---|---|---|
| `Cors`, `SecureHeaders`, `RequestIdPlugin`, `ErrorHandling`, `Tracing`, `ConfiguredTracing` | `()` | `add_layer` (Graph) |
| `Health` | `()` | `after_routes(register_routes)` |
| `AdvancedHealth` | `(HealthRegistry,)` | `after_routes` |
| `OpenApiPlugin` | `()` | `after_routes` (reads `RouteRegistry`) |
| `NormalizePath`-style | `()` | `wrap_router` (Finalize) |
| `DevReload` | `()` | as today, behind `dev-reload` |

`Tracing`/`ConfiguredTracing` install a subscriber — side effect in `setup()`
(pre-graph, runs even on `with_state(())`), which is the right slot.

**1f. Remove.** `Plugin` (post-state) trait, `.with()`, `should_be_last`,
`RawPreStatePlugin`'s "Raw" qualifier (rename to `PluginInstall`, still
`#[doc(hidden)]`). Rename `PreStatePlugin` → `Plugin`. Keep the
`#[diagnostic::on_unimplemented]` on `Plugin` and add one on the removed
method path: `.with()` stays as a `#[deprecated]`-free hard removal; the
compile error for `.with(X)` is "no method `with`" — acceptable given the
no-production rule, but `llm.txt` must be updated in the same PR.

### Steps

1. `contexts.rs` / `deferred.rs`: staged `EffectSet`, `after_routes`,
   `RoutesContext`; `build_state` + `typed.rs::build` drain per stage.
   Tests: `r2e-core/tests/plugin/lifecycle.rs` (stage order, install order
   within stage, disabled plugin drops Routes/Finalize but not shutdown).
2. `RouteRegistry` collected by `register_controller` (typed.rs) and modules
   fold; exposed on `RoutesContext`. Test: a plugin sees routes registered
   *after* it was installed.
3. `type Controllers` on `Plugin` + `Mods` entry in `plugin()`. Tests:
   `r2e-core/tests/plugin/controllers.rs` — controller injecting a `Provided`
   bean, controller injecting an app bean, trybuild
   `plugin_controller_dep_missing.rs` (guided error names the plugin).
4. Migrate builtins (1e), `HealthRegistry` (1d), OpenAPI (1c).
5. Remove post-state trait + `.with()`; rename `PreStatePlugin` → `Plugin`.
   Mechanical sweep: `rg "\.with\(" --type rust`, `rg PreStatePlugin`.
6. Docs: `docs/claude/plugins.md` (drop "Two plugin kinds", add stages +
   controllers), `docs/features/*`, `llm.txt` (every `.with(` example),
   `r2e-cli` templates, all `examples/*`.

### Breaking

`Plugin`/`.with()` removed; `PreStatePlugin` renamed; `should_be_last`
removed; `OpenApiPlugin`/`Health`/`Cors`/… move to `.plugin()` before
`build_state()`.

### Risks / open points

- `with_state(())` shortcut: Routes/Finalize effects never run there (same as
  Graph effects today) — keep the documented "graph path only" rule.
- Layer order for apps that relied on `.with(Cors)` being *after*
  controllers: layers wrap the whole router either way; only
  `ErrorHandling` (catch-panic) vs tracing order matters — pin it in a test.
- `r2e dev`: plugin controllers are volatile like plugin beans; the
  `forced_rebuild` seed already covers dependents. Route re-registration per
  cycle is what modules do today — no new gap.

---

## Plan 2 — Modules bring their plugins

### Goal

A feature module can *install* plugins, not only require them, so
`.register_module::<Billing>()` is sufficient for a module that needs
`Scheduler` + a datasource.

### Design

**Ownership rule — DECIDED 2026-08-26 (option A):** a plugin is installed by
exactly one owner — the app or one module. Other modules that need it use
`requires_plugins` (unchanged). Reason: type-level dedup ("install unless
already in `P`") needs specialization; the existing
`RequiredPluginInstalled<Plug, Idx>` can *prove presence* by index inference
but cannot branch on absence. Runtime dedup would silently drop a second,
differently-configured install — worse than an error. A double install stays
the existing `DuplicateBean` boot error, with the message extended to name
both owners and suggesting `requires_plugins`
(`plugin 'Scheduler' installed by app and by module 'Billing' — use requires_plugins(Scheduler) in the module`).

Alternatives considered and rejected: (B) runtime "first wins" dedup by
`TypeId` — silently discards one of two builder-configured instances
(`Executor::builder().max(64)` vs `Executor::default()`), and duplicates in
`P` make `HasBean`/`RequiredPluginInstalled` index inference ambiguous; (C)
dedup only when instances compare equal — needs `PartialEq`/fingerprint on
every plugin, impossible for closure/handle-carrying ones (`OidcServer`,
`AdvancedHealth`). Revisit B only if plugin config ever becomes YAML-only
(instance = marker), which would make A → B non-breaking.

Trait surface:

```rust
pub trait FeatureModule {
    // existing: Providers, Controllers, Exports, Imports, RequiredPlugins
    type Plugins: ModulePlugins = ();          // tuple of plugin types
    fn plugins() -> Self::Plugins { Default::default() }  // configured instances
}
```

Macro: `#[module(plugins(Scheduler::default(), Executor::builder().max(8).build()))]`
— expressions, not types; the macro infers `type Plugins` from the tuple of
expression types (`(<expr as _>, …)`) via a helper fn
`fn plugins() -> (A, B) { (expr_a, expr_b) }` with the types spelled by the
user when inference needs it: `plugins(Scheduler = Scheduler::default())`.
Recommendation: require the `Type = expr` form (unambiguous, greppable).

`register_module_impl`: fold `M::Plugins` through `plugin()` **first** (so
`P` grows by their `Provided` before the module's own providers are
scope-checked), then providers, controllers, exports as today. Modules'
`requires_plugins` keep pointing at `P`, so `register_module::<A>()` before
`register_module::<B>()` where B brings A's required plugin is still the
existing guided error — order matters, same as app installs.

Type-level: `ModulePlugins` is a tuple → HList fold of `RawPreStatePlugin`
installs; `ModuleRegistered<M, P, R, Mods>` output type must append the
plugins' `Provisions`/`Required` lists — extend the associated-type
computation in `registration.rs` (`RegisterModule`) with
`<M::Plugins as ModulePlugins>::Provisions`.

`Exports` may include a plugin-provided bean (e.g. a module that brings the
datasource and exports `DbPool<Postgres>`): `ExportsProvided` check must
accept `Provides ∪ PluginProvisions`.

### Steps

1. `module.rs`: `type Plugins` + `fn plugins()` + `ModulePlugins` tuple impls
   (arity macro like `impl_module_controllers`).
2. `registration.rs` / `nostate.rs`: fold plugins in `register_module_impl`;
   grow `P`/`R`; extend `ExportsProvided`.
3. `r2e-macros/src/attrs/module_attr.rs`: `plugins(Type = expr, …)` key;
   targeted errors for a bare type / missing `=`.
4. `DuplicateBean` message names the owners (`nostate.rs` registration
   backend records the owner label per plugin group node).
5. Tests: `r2e-core/tests/di/module.rs` — module brings Scheduler and its
   controller uses `#[scheduled]`; module exports a plugin bean; trybuild
   `module_plugin_double_install.rs` (app + module) and
   `module_plugin_required_before_brought.rs`.
6. Docs: `di-builder-refactor.md` § modules, `plugins.md` § "Module-declared
   required plugins" → "Modules and plugins" (require vs bring), `llm.txt`.

### Breaking

None strictly (new optional key); `FeatureModule` gains an associated type
with a default, so hand-written impls compile unchanged.

### Depends on

Nothing. Benefits from plan 1 (a module can bring `Health`/`OpenApi`) and is
what makes plan 4's datasource composable.

---

## Plan 3 — Bean-level startup observers (`#[on_start]`)

### Goal

Quarkus `void onStart(@Observes StartupEvent)`: any `#[bean]` or controller
core can run async, fallible code **after the whole graph and all
controllers are built and before the server listens**, ordered explicitly.
Replaces the pattern "put it in the app's `on_start` closure and dig the
bean out of the HList".

### Lifecycle after this plan (complete ladder)

```
build_state():   bean factories / plugin build (topological)
                 #[post_construct]        (per bean, inside the graph)
                 controller cores built → controller #[post_construct]
run()/serve():   #[on_start]  ← NEW (beans + controller cores, ordered, may Err)
                 builder .on_start(closure) (kept; runs after the #[on_start] set)
                 plugin on_serve hooks (ServeContext)
                 bind + accept
shutdown:        on_drain → stop accepting → plugin on_shutdown* →
                 controller #[pre_destroy] → bean #[pre_destroy] → .on_stop
```

### Design

- Attribute `#[on_start]` on `#[bean]` methods and `#[routes]` impls. Same
  signature rules as `#[post_construct]`: `&self`, optionally async, returns
  `()` or `Result<(), Box<dyn Error + Send + Sync>>`. `Err` aborts boot
  (propagated as `run()` error, like the builder `on_start`).
- Ordering: `#[on_start(order = N)]`, `i32`, default `0`, ascending; ties in
  registration order. Same knob name as the test-suite `order` (consistency);
  Quarkus `@Priority` semantics.
- Trait `OnStart` (mirror of `PreDestroy`): `fn on_start(&self) -> BoxFuture<Result<()>>`
  + `const ON_START_ORDER: i32`. `#[bean]` emits `impl OnStart` +
  `register_on_start` on `Registrable` (same hook as `register_pre_destroy`,
  `beans/registry_provide.rs`); controllers get `Controller::on_start(core)`
  (same shape as `Controller::pre_destroy`, `typed.rs` ~L612).
- Storage: `Vec<(i32, Box<dyn FnOnce() -> BoxFuture<Result<()>>>)>` on the
  typed builder, sorted stably before run. Beans read from the resolved
  graph (so a pinned override's hook — or lack of one — is what runs; same
  rule as `run_pre_destroy`).
- `TestApp` / `build_with_consumers`: **run** `#[on_start]` (Quarkus fires
  `StartupEvent` in `@QuarkusTest`; a seeding hook must run under test).
  Document the asymmetry with `#[pre_destroy]` (which needs a shutdown).
- Rejections (compile errors, as for `#[post_construct]`): combined with a
  route/`#[scheduled]`/`#[consumer]` marker, params, `#[intercept]`; more
  than one `#[on_start]` per impl is allowed (distinct methods), each
  ordered.
- Not in scope: `#[on_serve]` with `ServeContext` (shutdown token) — the
  plugin `on_serve` covers it; revisit if a bean needs the token (today
  `CancelToken` is a bean via `Deps`, which is enough).

### Steps

1. `r2e-core`: `OnStart` trait (`di/lifecycle.rs` next to `PreDestroy`),
   registry vec + `Registrable::register_on_start`, typed builder
   collection + sorted run in `run()` before `on_start` closures; `TestApp`
   boot path runs it.
2. `r2e-macros`: `extract/` parser for `on_start(order = N)`; `transverse.rs`
   emits the impl for beans and controller cores; rejection diagnostics.
3. Tests: `r2e-core/tests/di/lifecycle.rs` (order, Err aborts, pinned
   override skips, runs under `build_with_consumers`),
   `tests/controller/lifecycle.rs`, trybuild for rejections.
4. Docs: `docs/features/10-lifecycle-hooks.md` (ladder above),
   `docs/claude/beans-di.md`, CLAUDE.md `#[post_construct]`/`#[pre_destroy]`
   paragraph gains `#[on_start]`, `llm.txt`.

### Breaking

None. Builder `.on_start` stays.

---

## Plan 4 — Datasource plugin (`SqlxDataSource<DB>`) — the acceptance case

### Goal

```rust
AppBuilder::new()
    .load_config::<Root>()
    .plugin(SqlxDataSource::<Postgres>::new().migrations(&MIGRATOR))
    .plugin(AdvancedHealth::builder().build())
    .register_controllers::<(UserController,)>()
    .build_state().await
```

with `datasource.url` (live config), pool settings, and
`datasource.migrate-at-start: true` (Quarkus `quarkus.flyway.migrate-at-start`)
— migrations run inside `build`, boot fails if they fail, no `on_start`.

### Design

- Crate `r2e-data-sqlx`, `SqlxDataSource<DB, Tag = Default>`:
  `Provided = (DbPool<DB, Tag>,)` — `Tag` is a zero-sized marker for named
  datasources (Quarkus `quarkus.datasource."name".*`), `CONFIG_PREFIX` is
  `datasource` for `Default` and `datasource.<name>` otherwise
  (`Tag::NAME`). This is the newtype answer to "two Postgres pools".
  `DbPool<DB>` today has no `Tag` param — adding one with a default is the
  breaking part (`DbPool<DB>` keeps working via the default).
- `Config` (`#[derive(ConfigProperties)]`): `url: LiveConfig<String>`,
  `max-connections`, `min-connections`, `acquire-timeout`,
  `migrate-at-start: bool = false`, `migrations.locations` (unused by sqlx —
  `sqlx::migrate!` is compile-time; the app passes a `&'static Migrator`
  through the builder; config only gates *whether* it runs).
- `Deps = ()`; optional `Deps = (HealthRegistry,)` behind a builder flag or,
  cleaner, a second plugin `DataSourceHealth<DB>` with
  `Deps = (DbPool<DB>, HealthRegistry)` (plan 1d). Recommendation: the
  second plugin — keeps the datasource independent of Health.
- `build`: `DbPool::connect_with(cfg)` → if `migrate-at-start` and a
  `Migrator` was given → `migrator.run(&pool).await?` → `on_shutdown_async`
  closes the pool. `LiveConfig` URL rotation stays `DbPool`'s existing job.
- Tenant: `PerTenant::<DbPool<DB>>::from::<TenantPools<DB>>()` already
  exists; the plugin does not change it. Per-tenant migrations remain the
  documented deferral in `tenant.rs`.
- Diesel: `DieselDataSource<Conn>` mirror, `diesel_migrations::MigrationHarness`
  with `EmbeddedMigrations` passed the same way.
- With plan 1b, the plugin can ship an optional `#[controller]` (e.g.
  `/admin/db/migrations` status) — not in the first cut.
- With plan 2, `#[module(plugins(SqlxDataSource<Postgres> = SqlxDataSource::new().migrations(&MIGRATOR)), exports(DbPool<Postgres>))]`
  makes a self-contained persistence module.

### Steps

1. `DbPool<DB, Tag = Default>` + `Tag` marker trait; `TenantPools` untouched.
2. `SqlxDataSource` plugin + `DataSourceConfig`; `DataSourceHealth` (after
   plan 1d, otherwise skip).
3. Migrate `examples/example-postgres` and `example-multi-tenant-db` off
   `.provide(pool)` + `on_start` migrations.
4. Tests (`r2e-data/backends/sqlx/tests/`): boot with sqlite in-memory +
   embedded migrator; `migrate-at-start=false` skips; failing migration →
   `Plugin 'SqlxDataSource' failed to build`; named datasource pair; pinned
   `DbPool` override skips connect (`SKIP_BUILD_WHEN_ALL_PINNED = true`).
5. Docs: `docs/features/06-data-repository.md`, `subsystems.md`, `llm.txt`,
   `r2e new` template (`r2e-cli`).

### Breaking

`DbPool` gains a defaulted type param (source-compatible unless a user
spells `DbPool<DB>` in a position where defaults don't apply — impl blocks).

### Status (2026-08-26) — steps 1–5 shipped, health deferred

Shipped on `feat/plugin-module-lifecycle`: `SqlxDataSource<DB, Tag>` +
`DieselDataSource<Conn, Tag>`, tagged `DbPool`/`DbTx`, `datasource_tag!`,
`DataSourceConfig`, migrated `example-postgres`, tests
(`r2e-data/backends/{sqlx,diesel}/tests/datasource/`), docs + `llm.txt`.

Deviations from the design above:

- **`Deps = (LiveConfigRegistry,)`, not `()`.** `Config` cannot carry
  `url: LiveConfig<String>` (no `FromConfigValue` for a live handle), and the
  bean graph is empty at plugin-build time, so the plugin takes the registry as
  a real dep and mints the handle itself. The typed `url: Option<String>`
  stays, purely so a missing key fails with `` `datasource.url` is not set ``
  instead of an SQLx parse error.
- **`DataSourceHealth<DB>` is NOT implemented** — it needs `HealthRegistry`
  from plan 1d. Ship it with plan 1d, as a second plugin
  (`Deps = (DbPool<DB, Tag>, HealthRegistry)`), keeping the datasource
  independent of Health.
- **No `migrations.locations` key** — it was already noted as unused for sqlx,
  and Diesel's `embed_migrations!` is compile-time too. Omitted rather than
  accepted-and-ignored.
- **No `datasource.enabled` gate**: a pool bean has no inert form, so
  `enabled = false` warns and is ignored. The test-time replacement is
  `override_bean` + `SKIP_BUILD_WHEN_ALL_PINNED = true`.
- **`DataSourceTag` is per backend**, not hoisted into `r2e-core`: an app uses
  one backend, and a datasource marker is not runtime foundation. The trait
  carries both `NAME` and `CONFIG_PREFIX` because a `const` cannot concatenate
  `"datasource." + NAME` on stable; `datasource_tag!` mints the pair together.
- **`example-multi-tenant-db` was left alone** — it never used
  `.provide(pool)` + `on_start` migrations (it is the `PerTenant`/`TenantPools`
  path, which the plugin does not change), and the `r2e-cli` templates emit no
  pool producer either.
- The rotation watcher moved from `#[producer(start)]` to
  `ctx.on_serve(|serve| serve.track(ServiceComponent::start(pool, token)))`.

---

## Suggested order

Plan 4 (steps 1–2, without health) → Plan 3 → Plan 1 → Plan 2 → Plan 4
(health + module form). Plan 4 first because it is small, immediately
useful, and validates that `build()`-time migrations are the right slot
before the larger refactors; plan 3 is independent and small; plan 1 is the
big breaking one; plan 2 lands on top of plan 1's unified `Plugin`.
