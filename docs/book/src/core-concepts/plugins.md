# Plugins

Plugins extend R2E applications with reusable middleware, routes, and services. R2E ships with several built-in plugins and supports custom ones.

## Built-in plugins

Install plugins with `.plugin(p)` on the builder — **always before**
`build_state()`. There is only one plugin kind and one install call:

```rust
AppBuilder::new()
    .plugin(Health)
    .plugin(Cors::permissive())
    .plugin(HttpTrace::new())
    .plugin(ErrorHandling)
    .plugin(NormalizePath)
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Available plugins

| Plugin | Description |
|--------|-------------|
| `Health` | `GET /health` returning 200 "OK" |
| `Cors::permissive()` | Permissive CORS headers |
| `Cors::new(layer)` | Custom CORS configuration |
| `HttpTrace::new()` | Per-request span + summary line, request ids, exclusions (`trace.*` keys) |
| `HttpTrace::builder()…build()` | Same, configured in code (a builder knob beats the YAML) |
| `Tracing` | Installs the log **subscriber** only — redundant under `r2e::launch` / `#[r2e::main]` |
| `Tracing::configured(config)` | Configurable subscriber (format, ansi, thread IDs, etc.) |
| `Tracing::from_config(&r2e_config)` | Subscriber configured from YAML (`tracing.*` keys) |
| `ErrorHandling` | Catches panics, returns JSON 500 |
| `NormalizePath` | Trailing-slash normalization |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation |
| `SecureHeaders` | Security headers (X-Content-Type-Options, etc.) |
| `OpenApiPlugin` | OpenAPI spec + docs UI |
| `Prometheus` | Prometheus metrics at `/metrics` |
| `EmbeddedFrontend` | Embedded static file serving with SPA fallback (feature `static`) |

### Plugins that provide beans

Many plugins publish beans into the graph, which other beans and controllers
then `#[inject]`:

```rust
AppBuilder::new()
    .plugin(Executor)     // provides PoolExecutor (Scheduler runs ticks on it)
    .plugin(Scheduler)    // provides CancelToken + ScheduledJobRegistry
    .build_state()
    .await
    // ...
```

`Scheduler` **requires the `Executor` plugin**: it declares
`type Deps = (PoolExecutor,)`, so `.plugin(Scheduler)` without a `PoolExecutor`
in the graph fails at `build_state()` with the guided "missing
`.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. `Deps` are
checked against the final provision list, so the order between the two plugins does
not matter.

| Plugin | Description |
|-----------------|-------------|
| `Executor` | Managed task pool (`PoolExecutor`) with bounded concurrency and graceful drain |
| `Scheduler` | Background task scheduling runtime (requires `Executor`; ticks run on its pool) |

## Enabling and disabling plugins from config

Any plugin with a config section (a `CONFIG_PREFIX`) can be switched off from
YAML with `<prefix>.enabled: false` — no code change:

```yaml
prometheus:
  enabled: false
```

The default is `true`. A disabled plugin drops its *surface* effects (routes,
layers, serve hooks, stored data) — but **not** its cleanup hooks
(`on_shutdown`/`on_shutdown_async`): `build` ran, so whatever it constructed is
still disposed of at shutdown. And **its provided beans still exist** —
`build` runs with `ctx.enabled() == false` and returns a disabled variant, so
anything injecting the beans keeps working. See
[Custom Plugins](../advanced/custom-plugins.md#enabling-and-disabling-a-plugin-from-config)
for the full semantics.

## Feature modules and plugins

A feature module either **requires** a plugin someone else installs, or
**brings** the plugin itself.

### Require

Declaring the plugins a module depends on turns a missing plugin into a clear
compile error naming the plugin, instead of a confusing missing-bean error:

```rust
#[module(
    controllers(JobController),
    requires_plugins(Scheduler),
)]
pub struct JobsModule;
```

If `Scheduler` is not installed before `register_module::<JobsModule>()` — by
the app, by an earlier module, or by this module itself — the build fails with a
message pointing at `.plugin(Scheduler)`.

### Bring

A module that owns a piece of infrastructure can ship the plugin with it, so the
app only registers the module:

```rust
#[module(
    providers(ItemRepo),
    controllers(ItemController),
    exports(ItemRepo),
    plugins(SqlxDataSource<Sqlite> = SqlxDataSource::<Sqlite>::new()),
)]
pub struct DataModule;

AppBuilder::new()
    .register_module::<DataModule>()   // installs the datasource plugin too
```

Each entry is written `Type = expr`: the type grows the provision list at
compile time, the expression is the instance to install. Installing happens
exactly as a `.plugin(..)` at the `register_module` call site would — the
plugin's beans become available to the whole app (they need **no** `exports(..)`
entry, and must not be listed in one), its controllers are mounted, and its
effects apply at that position in install order.

### One owner per plugin

A plugin is installed by exactly one owner: the app **or** one module. Every
other module that merely needs it uses `requires_plugins`. Installing the same
plugin twice is a startup error naming both owners:

```text
Plugin 'Scheduler' is installed by app and by module 'JobsModule'.
A plugin has exactly one owner — the app or ONE module. Use
`requires_plugins(Scheduler)` in every module that only needs it, and keep the
single `.plugin(Scheduler)` / `plugins(Scheduler = ..)` install.
```

## Plugin ordering

Nothing has to be installed "last" any more. Every plugin registers its effects
into one of three stages, and the stage — not the install position — decides
when the effect is applied:

| Stage | What goes there | Applied |
|---|---|---|
| **Graph** | Tower layers, plugin data, serve hooks | right after the bean graph resolves |
| **Routes** | anything that needs the complete route table (OpenAPI) | after **every** controller is registered |
| **Finalize** | transport-level wraps (a gRPC multiplexer) | outermost, after every HTTP layer |

Within a stage, effects apply in install order, and a later layer ends up
**outside** an earlier one. Practical consequences:

- `NormalizePath` can be installed at any point: it is applied at build time as a pre-routing URI rewrite wrapping the whole router
- `EmbeddedFrontend` and `OpenApiPlugin` can be installed anywhere — their routes are mounted from a Routes-stage effect
- `HttpTrace` early keeps it inside (and therefore observed by) later layers
- Panic capture is installed by the framework regardless — innermost (so a handler panic is a plain 500 to the tracing and metrics layers) plus an outermost last-resort net; `ErrorHandling` only adds a redundant copy

## Custom Tower layers

For Tower middleware that doesn't need the full plugin API, use `.with_layer()`
on the built app:

```rust
use tower_http::timeout::TimeoutLayer;

AppBuilder::new()
    .build_state()
    .await
    .with_layer(TimeoutLayer::new(Duration::from_secs(30)))
    // ...
```

## Writing custom plugins

A plugin **is one async, fallible factory** for the beans it
provides: `build` runs inside `build_state()` as a node of the bean graph.
`Provided` is a **tuple** of beans — `(A,)` for one, `(A, B)` for several,
`()` for none — and `build` returns it. No builder generics needed:

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

pub struct MyPlugin;

impl Plugin for MyPlugin {
    type Provided = (MyConfig,);
    type Deps = ();          // no dependencies on other beans
    type Config = ();
    type Controllers = ();   // no controllers shipped by this plugin

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(MyConfig,), PluginBuildError> {
        Ok((MyConfig::default(),))
    }
}
```

`build` may await (connect to a backend, run migrations…) and may fail — an
`Err` aborts startup with an error naming the plugin. Every impl declares
`type Deps`; set it to `()` unless the plugin consumes an application bean.

### Consuming application beans

`Deps` names the beans the plugin builds from. They are **real edges in the
bean graph**: constructed before `build` runs and handed to it by value. Any
bean qualifies — `.provide()`-d, factory-built (`.register::<T>()`), or
provided by another plugin. `Deps` is verified against the **final** provision
list at `build_state()`, so the order between `.plugin()`, `.provide()`, and
`.register()` calls does not matter; a missing dep is a guided compile error
("missing `.provide::<X>()` or `.register::<X>()`").

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

pub struct MetricsExporter;

impl Plugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    type Deps = (MetricsRegistry,);   // factory-built is fine
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (registry,): (MetricsRegistry,),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ExporterHandle,), PluginBuildError> {
        let handle = ExporterHandle::connect().await?;
        let h = handle.clone();
        ctx.on_serve(move |_sc| h.bind(registry));
        Ok((handle,))
    }
}

// `MetricsRegistry` may be registered AFTER the plugin — `Deps` is checked
// against the final provision list at `build_state()`, not at the call site.
AppBuilder::new()
    .plugin(MetricsExporter)
    .register::<MetricsRegistry>()
    .build_state().await
```

Because deps arrive before `build`, a provided bean that depends on another
bean is constructed **directly** — there is no shell/fill pattern anymore.
(`r2e::Late<T>` still exists as an escape hatch for genuinely post-boot
fills.)

### Effects: layers, hooks, plugin data

Side effects are registered on the `PluginBuildContext` during `build` — plain
closures, no `Box`:

```rust
async fn build(
    self,
    _deps: (),
    _config: Option<()>,
    ctx: &mut PluginBuildContext,
) -> Result<(MyToken,), PluginBuildError> {
    let token = MyToken::new();
    let t = token.clone();
    ctx.add_layer(|router| router);              // Tower layer
    ctx.on_serve(|_serve_ctx| { /* server starting */ });
    ctx.on_shutdown(move || { t.cancel(); });
    Ok((token,))
}
```

`PluginBuildContext` provides:
- `enabled()` — the `<prefix>.enabled` config gate (default `true`)
- `graph()` — a weak `GraphHandle` on the final resolved graph (fills at the end of a successful `build_state()`; stays readable for the app's whole life and for any tracked task that outlives it — the router, each tracked task and the serving scope own the graph independently, so reads only go `None` once the last owner is gone)
- `config_raw()` — the loaded `R2eConfig`, if any
- `add_layer()` — Tower layer (**Graph** stage)
- `store_data()` — type-keyed plugin data for cross-plugin coordination (**Graph**)
- `on_serve()` — serve-time hook (**Graph**)
- `after_build()` — boot-time escape hatch with full-graph access (**Graph**)
- `after_routes()` — runs once **every** controller is registered; read the route
  registry (`routes.routes()`) and mount routers from it (**Routes** stage)
- `wrap_router()` — replace the whole router, outside every HTTP layer (**Finalize**)
- `on_shutdown()` / `on_shutdown_async()` — cleanup hooks (never gated)

Effects are buffered and applied per stage, **in plugin install order** within a
stage (builds themselves run in dependency order). Disabling the plugin via
`<prefix>.enabled: false` drops **all three** surface stages but keeps the
*cleanup* hooks (`on_shutdown`, `on_shutdown_async`) — `build` still runs, so the
beans exist and whatever they hold still has to be released; check
`ctx.enabled()` and return a cheap, inert disabled variant.

### Shipping controllers from a plugin

A plugin can declare `#[controller]` types instead of hand-assembling a
`Router`, which gets it guards, `#[roles]`, OpenAPI metadata and extractors for
free:

```rust
#[controller(path = "/metrics")]
pub struct MetricsController {
    #[inject] registry: MetricsRegistry,   // the plugin's own provided bean
}

impl Plugin for Metrics {
    type Provided = (MetricsRegistry,);
    type Deps = ();
    type Config = ();
    type Controllers = (MetricsController,);
    // ...
}
```

Plugin controllers are registered by `build_state()`, so their routes are part
of the route registry every `after_routes` effect sees. Their `#[inject]` fields
are checked against the final provision list at `build_state()` — a missing bean
is a compile error naming the plugin.

There is also a rare pre-graph hook, `fn setup(&mut self, &mut
PluginSetupContext)` (default no-op), for the few things that must happen at
`.plugin()` time — e.g. `store_data` that other subsystems read even when the
plugin is disabled. It cannot register layers, routes or hooks: setup actions
are ungated, so allowing them would let a disabled plugin serve traffic.

The lower-level `PluginInstall` trait (`#[doc(hidden)]`, HList-based) still
backs `.plugin()` via a blanket impl, but you only need to implement it directly
when a plugin must call arbitrary builder methods (`.register()`, `.provide()`,
…) itself — a rare escape hatch.
