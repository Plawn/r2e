# Plugins

Plugins extend R2E applications with reusable middleware, routes, and services. R2E ships with several built-in plugins and supports custom ones.

## Built-in plugins

Install plugins with `.with(plugin)` on the builder (after `build_state()`):

```rust
AppBuilder::new()
    .build_state()
    .await
    .with(Health)
    .with(Cors::permissive())
    .with(Tracing)
    .with(ErrorHandling)
    .with(NormalizePath)
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
| `Tracing` | Request tracing via `tracing` + `tower-http` (default config) |
| `Tracing::configured(config)` | Configurable tracing (format, ansi, thread IDs, etc.) |
| `Tracing::from_config(&r2e_config)` | Tracing configured from YAML (`tracing.*` keys) |
| `ErrorHandling` | Catches panics, returns JSON 500 |
| `NormalizePath` | Trailing-slash normalization |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation |
| `SecureHeaders` | Security headers (X-Content-Type-Options, etc.) |
| `OpenApiPlugin` | OpenAPI spec + docs UI |
| `Prometheus` | Prometheus metrics at `/metrics` |
| `EmbeddedFrontend` | Embedded static file serving with SPA fallback (feature `static`) |

### Pre-state plugins

Some plugins need to install before `build_state()`. Use `.plugin()` instead of `.with()`:

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

| Pre-state Plugin | Description |
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

## Requiring a plugin from a feature module

A feature module can declare the plugins it depends on so a missing plugin is a
clear compile error naming the plugin, instead of a confusing missing-bean error:

```rust
#[module(
    controllers(JobController),
    requires_plugins(Scheduler),
)]
pub struct JobsModule;
```

If `Scheduler` is not `.plugin(Scheduler)`-ed before `register_module::<JobsModule>()`,
the build fails with a message pointing at `.plugin(Scheduler)`.

## Plugin ordering

Plugins are installed in registration order. Some have ordering requirements:

- `NormalizePath` can be installed at any point: it is applied at build time as a pre-routing URI rewrite wrapping the whole router
- `EmbeddedFrontend` should be installed last (plugins may use the `should_be_last()` hint — R2E warns if plugins are added after one that sets it)
- `Tracing` should be early to capture all requests
- `ErrorHandling` should be after `Tracing` but before route registration

## Custom Tower layers

For Tower middleware that doesn't need the full plugin API, use `.with_layer()`:

```rust
use tower_http::timeout::TimeoutLayer;

AppBuilder::new()
    .build_state()
    .await
    .with_layer(TimeoutLayer::new(Duration::from_secs(30)))
    // ...
```

## Writing custom plugins

### Post-state plugins

Implement the `Plugin` trait for plugins that install after `build_state()`:

```rust
use r2e::prelude::*; // Plugin, AppBuilder

pub struct MyPlugin;

impl Plugin for MyPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        // Add routes, layers, or middleware
        app.register_routes(Router::new().route("/my-endpoint", get(|| async { "Hello from plugin" })))
    }
}
```

`should_be_last()` (default `false`) marks plugins that must be the outermost
layer — the builder warns if anything is installed after one.

### Pre-state plugins

A pre-state plugin **is one async, fallible factory** for the beans it
provides: `build` runs inside `build_state()` as a node of the bean graph.
`Provided` is a **tuple** of beans — `(A,)` for one, `(A, B)` for several,
`()` for none — and `build` returns it. No builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};

pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    type Provided = (MyConfig,);
    type Deps = ();          // no dependencies on other beans
    type Config = ();

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
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};

pub struct MetricsExporter;

impl PreStatePlugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    type Deps = (MetricsRegistry,);   // factory-built is fine
    type Config = ();

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
- `add_layer()` / `wrap_router()` — router layers / outermost router transform
- `store_data()` — type-keyed plugin data for post-state coordination
- `on_serve()` / `on_shutdown()` / `on_shutdown_async()` — lifecycle hooks
- `after_build()` — boot-time escape hatch with full-graph access

Effects are buffered and applied after the graph resolves, **in plugin install
order** (builds themselves run in dependency order). Disabling the plugin via
`<prefix>.enabled: false` drops the *surface* effects (layers, routes,
`on_serve`, plugin data) but keeps the *cleanup* ones (`on_shutdown`,
`on_shutdown_async`) — `build` still runs, so the beans exist and whatever they
hold still has to be released; check `ctx.enabled()` and return a cheap, inert
disabled variant.

There is also a rare pre-graph hook, `fn setup(&mut self, &mut
PluginSetupContext)` (default no-op), for the few things that must happen at
`.plugin()` time — e.g. `store_data` that other subsystems read even when the
plugin is disabled. It cannot register layers, routes or hooks: setup actions
are ungated, so allowing them would let a disabled plugin serve traffic.

The lower-level `RawPreStatePlugin` trait (`#[doc(hidden)]`, HList-based) still
backs `.plugin()` via a blanket impl, but you only need to implement it directly
when a plugin must call arbitrary builder methods (`.register()`, `.provide()`,
…) itself — a rare escape hatch.
