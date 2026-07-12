# Plugin System Reference

Authoritative reference for R2E's plugin system as of the plugin DX/DI
overhaul (phases 1–6, PR #29). Source of truth: `r2e-core/src/plugin.rs`,
`r2e-core/src/type_list.rs` (`PluginDeps` / `PluginProvisions`),
`r2e-core/src/config/mod.rs` (`PluginConfig`).

## Two plugin kinds

| | Pre-state | Post-state |
|---|---|---|
| Trait | `PreStatePlugin` | `Plugin` |
| Install call | `.plugin(p)` **before** `build_state()` | `.with(p)` **after** `build_state()` |
| Can provide beans | yes (tuple `Provided`) | no |
| Typical use | Scheduler, Prometheus, OIDC, gRPC, Executor | Health, Cors, OpenApi, NormalizePath |

Passing one to the other's install method is a guided compile error
(`#[diagnostic::on_unimplemented]` on `Plugin`, `PreStatePlugin`, and
`RawPreStatePlugin`). `Plugin` also has advisory `should_be_last()` — the
builder warns if another post-state plugin is added after one that returns
`true` (e.g. `NormalizePath`).

## PreStatePlugin surface

```rust
impl PreStatePlugin for MyPlugin {
    type Provided = (HandleA, HandleB);   // tuple: (A,), (A, B), or () — never a bare type
    type Deps     = (DbPool,);            // resolved at .plugin() time (pre-state)
    type LateDeps = (AppService,);        // resolved after build_state() (full graph)
    type Config   = MyPluginConfig;       // or (); typed file-config section
    const CONFIG_PREFIX: Option<&'static str> = Some("my_plugin");

    fn install(&mut self, (pool,): (DbPool,), ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        ctx.add_layer(|router| router.layer(...));   // sugar — see ordering below
        ctx.on_shutdown(|| { ... });
        (HandleA::new(pool), HandleB::new())
    }

    fn configure(
        self,                                  // by value: builder fields still on self
        (a, _b): &Self::Provided,              // the plugin's OWN instances (see pin-override note)
        (svc,): Self::LateDeps,                // any bean, incl. .register()-ed
        config: Option<Self::Config>,          // None if section absent / no config loaded
        ctx: &mut DeferredContext<'_>,
    ) { ... }
}
```

### Lifecycle

```
.plugin(Me)                 build_state()                          (serve)
     │                           │                                    │
     ▼                           ▼                                    ▼
 install(&mut self, Deps)   [bean graph built]                   on_serve hooks
   Provided → registry      then deferred actions run, per plugin,
                            in install order:
                            [A.explicit…, A.sugar, A.configure,
                             B.explicit…, B.sugar, B.configure]
                            configure(self, &Provided, LateDeps, Config)
```

There is **no** "all installs, then all configures" phase separation: deferred
work is grouped per plugin. A layer added from A's `configure` is applied
before (nested inside) a layer added at install time by a later plugin B.

### `Deps` vs `LateDeps` decision rule

- `Deps` — pre-built infrastructure handed to `.provide(instance)` *before*
  the `.plugin()` call. Checked at the call site (`AllSatisfied`); resolved at
  install. A `.register()`-ed type in `Deps` panics at runtime with a message
  steering to `LateDeps` (type-level fix was evaluated and deferred — tagging
  `P` would churn `Contains`/`AllSatisfied` everywhere).
- `LateDeps` — everything else: factory-built beans (`.register::<T>()`),
  beans from other plugins, beans registered *after* this plugin. Appended to
  the builder's requirement list `R` via `RawPreStatePlugin::AllRequired` and
  verified against the **final** provision list at `build_state()` (missing →
  the standard guided "missing `.provide::<X>()` or `.register::<X>()`"
  compile error). Resolved via `PluginDeps::resolve_from_context` from the
  materialized `BeanContext`.

### Tuple `Provided` / `PluginProvisions`

`Provided` is always a tuple, mapped to the type-level provision list by
`PluginProvisions` (arities 0–8, mirror of `PluginDeps`; no scalar impl — a
bare `type Provided = MyBean` gets an on_unimplemented pointing to `(MyBean,)`).
Values are deposited via `BeanRegistry::provide` — the exact `AppBuilder::provide`
path — so pin-override (`override_bean`) / last-wins semantics are identical;
the type-level list advances by one `with_updated_types()` phantom cast.

### Pin-override contract on `configure`

`configure`'s `provided` argument is a copy of exactly what `install` returned
(the plugin owns what it built). If a test pins an override for a `Provided`
type, the state/`BeanContext` hold the override but `provided` does **not**.
To see the bean as the app sees it, read it through the graph (`LateDeps` or
`ctx.bean_context()`). Locked by
`configure_provided_arg_keeps_own_instance_under_pin_override`.

### Typed `Config` (phase 4)

- `type Config` must implement `PluginConfig` (`r2e-core/src/config/mod.rs`):
  implemented for `()` (no config) and blanket for any `ConfigProperties` — a
  `#[derive(ConfigProperties)]` struct is a valid `Config` as-is.
- Loaded at **configure** time (the only point where config is guaranteed
  loaded; `.plugin()` may legitimately precede `load_config`). Rules
  (`load_plugin_config`): `None` when `CONFIG_PREFIX` is `None`, no config was
  loaded, or no key lives under the prefix; a present-but-invalid section
  **panics at boot** with the same `ConfigValidationError` report as a
  controller `#[config(section)]` mismatch, naming the plugin.
- Precedence convention (Prometheus is the reference implementation —
  `r2e-prometheus`, section `prometheus.*`): explicit builder setting > file
  config > built-in default. Merge happens in `configure` — which is why the
  plugin instance travels there by value.

### Install-context sugar vs `add_deferred`

`PluginInstallContext` sugar (`add_layer`, `wrap_router`, `store_data`,
`on_serve`, `on_shutdown`, `on_shutdown_async`) takes plain closures, buffers
them, and flushes them as ONE `DeferredAction` named after the plugin type
(`plugin_action_name`, `#[doc(hidden)]`). Ordering: explicit
`add_deferred` actions first (call order), then the single sugar action.
`DeferredContext` (what deferred actions and `configure` receive) has the same
surface plus `bean_context()` and boxed-closure variants.

`store_data` / plugin data: type-keyed storage that survives into controller
registration and serve hooks (`app.get_plugin_data::<T>()`); this is how the
gRPC plugin coordinates with `register_grpc_service`, and the Scheduler with
`#[scheduled]` task collection.

## Bean lifecycle hooks for `Provided` beans (phase 5)

A plugin's `Provided` values are deposited straight into the graph, so by
default they run no `PostConstruct` and no disposal. Opt them in **explicitly**
from `install` (no trait detection on stable):

```rust
fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyBean,) {
    ctx.run_post_construct::<MyBean>();  // MyBean: PostConstruct
    ctx.run_pre_destroy::<MyBean>();     // MyBean: PreDestroy
    (MyBean::new(),)
}
```

- `run_post_construct::<T>()` — fires during `build_state()`, **after every
  factory-bean post-construct**, through the same `BeanError::PostConstruct`
  path (a failure panics at boot).
- `run_pre_destroy::<T>()` — runs during graceful shutdown, in the **async
  shutdown phase after the plugin's own `on_shutdown_async` hooks**, in reverse
  registration order among bean disposers.
- **Both hooks read `T` from the resolved graph by type**, so a pinned override
  (`override_bean`) is the value they act on — same contract as everything else
  the graph holds (contrast the `configure` `provided` arg, which is the
  plugin's own copy). Backed by `BeanRegistry::register_provided_post_construct`
  / `register_pre_destroy`; the plain-`.provide()` equivalents are
  `AppBuilder::provide_with_post_construct` / `provide_with_pre_destroy`. See
  `docs/claude/beans-di.md` (Lifecycle for `.provide()`-d / plugin beans) and
  the tests in `r2e-core/tests/plugin.rs`.

## Conditional plugins: `<prefix>.enabled` (phase 6)

`.when()` cannot wrap `.plugin()` (the builder's type parameters change), so
conditionality is **runtime + config-driven**, Quarkus-style — the type-level
provision list stays fixed. For a plugin with `CONFIG_PREFIX = Some(prefix)`, the
boolean key `<prefix>.enabled` (default **true**) gates the plugin's **post-state
effects**. When `<prefix>.enabled = false`:

- its flushed sugar action (layers / `wrap_router` / `store_data` / `on_serve` /
  `on_shutdown` hooks) is **skipped**;
- its explicit `add_deferred` actions are **skipped** too (consistent choice —
  everything the plugin scheduled for post-state is gated as one);
- its `configure` is **skipped**;
- **its `Provided` beans still exist in the graph.** The type-level provision
  list is fixed at compile time, so disabling a plugin never removes its beans —
  `state.get::<ProvidedBean>()` still resolves. Code injecting a disabled
  plugin's bean keeps compiling and running; only the plugin's wiring is inert.
  **Author obligation:** a provided bean must therefore stay *usable* (no
  panic) even when `configure` never ran. If the bean is a handle to state that
  `configure` initializes, degrade gracefully — Prometheus is the reference:
  `PrometheusRegistry` accessors lazily default-initialize the global registry
  (real and registrable, just not exported at `/metrics`), and `configure`
  warns if it finds the registry pre-initialized.

**Not gated:**

- **`install` itself** — it already ran pre-state, before config is guaranteed
  loaded. In-tree plugins have no install-time side effects beyond bean
  construction; a plugin that *must* be fully inert when disabled should keep
  install cheap and put real work in `configure` / sugar (as Prometheus does).
- **Lifecycle hooks** (`run_post_construct` / `run_pre_destroy` registrars) —
  they act on beans that still exist, and other code may inject those beans, so
  their `PostConstruct` / `PreDestroy` still run. Keeps the bean lifecycle
  honest regardless of the flag.

**Mechanics.** Both the sugar-flush action and the `configure` action run inside
`build_state()` where the loaded config is available (`DeferredContext.config`),
so the gate lives generically in the blanket `RawPreStatePlugin` impl
(`plugin_config_enabled` + `gate_on_enabled` in `r2e-core/src/plugin.rs`). The
default is **enabled** whenever the prefix is `None`, no config was loaded, or
the `<prefix>.enabled` key is absent. When a plugin is disabled, a single
`tracing::info` is emitted (from the configure action — exactly one per plugin).

Reference implementation: **Prometheus** — `prometheus.enabled: false` mounts no
`/metrics` route and installs no tracking layer (all in `configure`), while the
`PrometheusRegistry` bean stays in the graph. See `r2e-prometheus/tests/plugin.rs`
and the enabled-gate tests in `r2e-core/tests/plugin.rs`
(`plugin_enabled_true_by_default_runs_all_effects`,
`plugin_enabled_false_skips_effects_but_keeps_beans`,
`plugin_enabled_false_without_config_loaded_is_enabled`).

## Module-declared required plugins (phase 6)

A feature module (`#[module]` / `register_module::<M>()`) whose beans or
controllers rely on a plugin-provided bean (e.g. `ScheduledJobRegistry` from the
Scheduler) can declare the requirement so a missing plugin is a **clear compile
error naming the plugin**, not an opaque missing-bean error on one of the
plugin's internal handle types:

```rust
#[module(
    controllers(JobController),
    requires_plugins(Scheduler),   // ← must be .plugin(Scheduler)-ed before this module
)]
pub struct JobsModule;
```

Or, hand-written, via [`FeatureModule::RequiredPlugins`] — a **tuple** of
pre-state plugin types (`()` for none):

```rust
impl FeatureModule for JobsModule {
    type Providers = TNil;
    type Controllers = (JobController,);
    type Exports = TNil;
    type Imports = TNil;
    type RequiredPlugins = (Scheduler,);
}
```

**Semantics.** At `register_module` the compiler checks that **every provided
bean of each required plugin** is already in the app-global provision list `P` —
i.e. the plugin was `.plugin(..)`-ed *before* the module. This reuses the plugin
type's own `RawPreStatePlugin::Provisions` and the existing `AllSatisfied` /
`Contains` machinery (`r2e-core/src/module.rs`:
`RequiredPluginInstalled` / `RequiredPluginsInstalled`, arities 0–8).

**Diagnostic.** `RequiredPluginInstalled`'s blanket impl carries
`#[diagnostic::do_not_recommend]` so the compiler surfaces the trait's own
`#[diagnostic::on_unimplemented]` message ("this feature module requires the
`Scheduler` plugin, which is not installed before it … install it with
`.plugin(Scheduler)`") instead of the inner "type `ScheduledJobRegistry` was not
provided" error. Covered by `compile-fail/module_required_plugin_not_installed.rs`
and `compile-pass/module_required_plugin_installed.rs`.

Note this is a **call-site** check (plugin must precede the module), stronger and
earlier than declaring the individual bean in `Imports` (which is verified at
`build_state()`). Use `requires_plugins` for the plugin-named diagnostic;
`imports` remains for individual beans from the app or other modules.

## RawPreStatePlugin (hidden escape hatch)

`#[doc(hidden)]`. HList-typed full-builder-access form that `.plugin()`
dispatches on; every `PreStatePlugin` gets it via the blanket impl
(`Provisions = Provided::AsList`, `Required = Deps::AsList` checked at call
site, `AllRequired = Deps ++ LateDeps` appended to `R`). Implement directly
only to drive arbitrary builder methods during install — no in-tree
implementor remains.

## Testing plugins

- Unit: build with `AppBuilder::new().plugin(X).build_state().await`, assert
  beans via `state.get::<T>()` / `app.bean_context()`; drive deferred hooks
  via the patterns in `r2e-core/tests/plugin.rs`.
- Config: `with_config` an in-memory `R2eConfig`; see the phase-4 tests in
  `r2e-core/tests/plugin.rs` and `r2e-prometheus/tests/plugin.rs` (precedence
  + validation-panic cases).
- Serve-path e2e per plugin is roadmap item W4.
