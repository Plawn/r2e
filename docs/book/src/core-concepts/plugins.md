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
    .plugin(Scheduler)    // provides CancellationToken + ScheduledJobRegistry
    .build_state()
    .await
    // ...
```

| Pre-state Plugin | Description |
|-----------------|-------------|
| `Scheduler` | Background task scheduling runtime |

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
    type Deps = ();
    type LateDeps = ();      // no post-state dependencies
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (MyConfig,) {
        (MyConfig::default(),)
    }
}
```

Every impl declares `type LateDeps` — set it to `()` unless the plugin consumes
an application bean after `build_state()` (see
[Consuming application beans](#consuming-application-beans)).

Plugins can declare typed dependencies via `Deps`. The compiler verifies at each `.plugin()` call that all deps have already been provided:

```rust
impl PreStatePlugin for MyPlugin {
    type Provided = (MyService,);
    type Deps = (DbPool, CancellationToken);
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (pool, token): (DbPool, CancellationToken), _ctx: &mut PluginInstallContext<'_>) -> (MyService,) {
        (MyService::new(pool, token),)
    }
}
```

`Deps` must be `.provide(instance)` values — `install` runs before the bean
graph exists. To consume a factory-built bean (`.register::<T>()`) or a bean
another plugin provides, use `LateDeps` + `configure` (next-but-one section).

### Consuming application beans

A pre-state plugin has a **two-stage lifecycle**: `install` (before
`build_state()`) and `configure` (after it). `Deps` resolve at install time and
can only name `.provide()`-d beans; `LateDeps` resolve in `configure` from the
fully materialized bean graph, so they can name **any** bean — factory-built
(`.register::<T>()`) or provided by another plugin.

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredContext};

pub struct MetricsExporter;

impl PreStatePlugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    type Deps = ();
    type LateDeps = (MetricsRegistry,);   // factory-built; not available at install
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (ExporterHandle,) {
        (ExporterHandle::new(),)
    }

    fn configure(
        self,
        (handle,): &(ExporterHandle,),
        (registry,): (MetricsRegistry,),
        _config: Option<()>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let handle = handle.clone();
        ctx.on_serve(move |_sc| handle.bind(registry));
    }
}

// `MetricsRegistry` may be registered AFTER the plugin — `LateDeps` is checked
// against the final provision list at `build_state()`, not at the call site.
AppBuilder::new()
    .plugin(MetricsExporter)
    .register::<MetricsRegistry>()
    .build_state().await
```

`configure` gets a borrowed copy of the plugin's `Provided`, the resolved
`LateDeps`, and a `DeferredContext` (same surface as deferred actions). Its
default is a no-op. **Rule:** `Deps` = pre-built infrastructure you `.provide()`;
`LateDeps` = anything else, including factory-built beans.

### Deferred actions

For plugins that need to perform setup after state construction, call the
context's sugar methods directly — pass plain closures, no `Box`, no
`DeferredAction`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

impl PreStatePlugin for MyPlugin {
    type Provided = (MyToken,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyToken,) {
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
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (TokenA, TokenB) {
        (TokenA::new(), TokenB::new())
    }
}
```

The lower-level `RawPreStatePlugin` trait (`#[doc(hidden)]`, HList-based) still
backs `.plugin()` via a blanket impl, but you only need to implement it directly
when a plugin must call arbitrary builder methods (`.register()`, `.provide()`,
…) itself — a rare escape hatch.
