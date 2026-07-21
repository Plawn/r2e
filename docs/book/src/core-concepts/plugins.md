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
    .plugin(Scheduler)    // provides CancellationToken + ScheduledJobRegistry
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

The default is `true`. A disabled plugin skips its post-state wiring (routes,
layers, serve/shutdown hooks, and its `configure` step), but **its provided beans
still exist** — anything injecting them keeps working. See
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

### Pre-state plugins (simple path)

Implement `PreStatePlugin` for plugins that provide beans. `Provided` is a
**tuple** of beans — `(A,)` for one, `(A, B)` for several, `()` for none — and
`install` returns that tuple. No builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    type Provided = (MyConfig,);
    type Deps = ();          // no dependencies on other beans
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (MyConfig,) {
        (MyConfig::default(),)
    }
}
```

Every impl declares `type Deps` — set it to `()` unless the plugin consumes
an application bean (see
[Consuming application beans](#consuming-application-beans)).

### Consuming application beans

A pre-state plugin has a **two-stage lifecycle**: `install` (before
`build_state()`) and `configure` (after it). `install` runs pre-state and never
sees resolved beans. `Deps` is the plugin's single dependency list: it is
appended to the builder's requirement list and verified against the **final**
provision list at `build_state()` — nothing is checked at the `.plugin()` call
site, so the order between `.plugin()`, `.provide()`, and `.register()` calls
does not matter. `Deps` can name **any** bean — provided, factory-built
(`.register::<T>()`), or provided by another plugin. A missing dep is a guided
compile error ("missing `.provide::<X>()` or `.register::<X>()`"). The resolved
beans are passed **by value** to `configure`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredContext};

pub struct MetricsExporter;

impl PreStatePlugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    type Deps = (MetricsRegistry,);   // factory-built is fine — resolved post-state
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (ExporterHandle,) {
        (ExporterHandle::new(),)
    }

    fn configure(
        self,
        (handle,): &(ExporterHandle,),
        (registry,): Self::Deps,
        _config: Option<()>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let handle = handle.clone();
        ctx.on_serve(move |_sc| handle.bind(registry));
    }
}

// `MetricsRegistry` may be registered AFTER the plugin — `Deps` is checked
// against the final provision list at `build_state()`, not at the call site.
AppBuilder::new()
    .plugin(MetricsExporter)
    .register::<MetricsRegistry>()
    .build_state().await
```

`configure` gets a borrowed copy of the plugin's `Provided`, the resolved
`Deps` (by value), and a `DeferredContext` (same surface as deferred actions).
Its default is a no-op.

**Provided bean needs a dep?** `install` cannot construct it (deps are not
resolved yet). Provide a **shell** over `r2e::Late<T>` — a `Clone`, Arc-shared,
first-write-wins write-once cell — and fill it post-state: sync via
`late.fill(value)` in `configure`, or async via
`ctx.run_post_construct::<Bean>()` (awaited inside `build_state()`). Read with
`Late::get() -> Option<&T>` or `Late::expect("what")`. See
[Custom Plugins](../advanced/custom-plugins.md#compile-time-dependency-checking)
for a worked example.

### Deferred actions

For plugins that need to perform setup after state construction, call the
context's sugar methods directly — pass plain closures, no `Box`, no
`DeferredAction`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

impl PreStatePlugin for MyPlugin {
    type Provided = (MyToken,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (MyToken,) {
        let token = MyToken::new();
        let t = token.clone();
        ctx.add_layer(|router| router);
        ctx.on_serve(|_serve_ctx| { /* run when server starts */ });
        ctx.on_shutdown(move || { t.cancel(); });
        (token,)
    }
}
```

`PluginInstallContext` provides:
- `add_layer()` — add a Tower layer
- `wrap_router()` — add an outermost transport-level router transform
- `store_data()` — store data in the builder
- `on_serve()` — register a serve hook
- `on_shutdown()` / `on_shutdown_async()` — register a shutdown hook

These calls are buffered and flushed as a single deferred action after
`build_state()`. For advanced control, `ctx.add_deferred(DeferredAction::new(..))`
is the low-level escape hatch (it runs before the buffered sugar action).

### Multiple provided beans

To provide **multiple** beans, widen the `Provided` tuple and return all of
them — still on the simple `PreStatePlugin` path, no builder generics:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

pub struct MultiProvider;

impl PreStatePlugin for MultiProvider {
    type Provided = (TokenA, TokenB);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (TokenA, TokenB) {
        (TokenA::new(), TokenB::new())
    }
}
```

The lower-level `RawPreStatePlugin` trait (`#[doc(hidden)]`, HList-based) still
backs `.plugin()` via a blanket impl, but you only need to implement it directly
when a plugin must call arbitrary builder methods (`.register()`, `.provide()`,
…) itself — a rare escape hatch.
