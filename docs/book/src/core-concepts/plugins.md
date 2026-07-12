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
use r2e::prelude::*; // Plugin, Router

pub struct MyPlugin;

impl<S: Clone + Send + Sync + 'static> Plugin<S> for MyPlugin {
    fn install(self, router: Router<S>) -> Router<S> {
        // Add routes, layers, or middleware
        router.route("/my-endpoint", get(|| async { "Hello from plugin" }))
    }

    fn should_be_last(&self) -> bool {
        false
    }
}
```

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

    fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (MyConfig,) {
        (MyConfig::default(),)
    }
}
```

Plugins can declare typed dependencies via `Deps`. The compiler verifies at each `.plugin()` call that all deps have already been provided:

```rust
impl PreStatePlugin for MyPlugin {
    type Provided = (MyService,);
    type Deps = (DbPool, CancellationToken);

    fn install(self, (pool, token): (DbPool, CancellationToken), _ctx: &mut PluginInstallContext<'_>) -> (MyService,) {
        (MyService::new(pool, token),)
    }
}
```

### Deferred actions

For plugins that need to perform setup after state construction, use `DeferredAction` via the context:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use r2e::plugin::DeferredContext;

impl PreStatePlugin for MyPlugin {
    type Provided = (MyToken,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyToken,) {
        let token = MyToken::new();
        let t = token.clone();
        ctx.add_deferred(DeferredAction::new("my-plugin", move |dctx: &mut DeferredContext| {
            dctx.add_layer(Box::new(|router| router));
            dctx.on_serve(|_serve_ctx| { /* run when server starts */ });
            dctx.on_shutdown(move || { t.cancel(); });
        }));
        (token,)
    }
}
```

`DeferredContext` provides:
- `add_layer()` — add a Tower layer
- `store_data()` — store data in the builder
- `on_serve()` — register a serve hook
- `on_shutdown()` — register a shutdown hook

### Multiple provided beans

To provide **multiple** beans, widen the `Provided` tuple and return all of
them — still on the simple `PreStatePlugin` path, no builder generics:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

pub struct MultiProvider;

impl PreStatePlugin for MultiProvider {
    type Provided = (TokenA, TokenB);
    type Deps = ();

    fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (TokenA, TokenB) {
        (TokenA::new(), TokenB::new())
    }
}
```

The lower-level `RawPreStatePlugin` trait (`#[doc(hidden)]`, HList-based) still
backs `.plugin()` via a blanket impl, but you only need to implement it directly
when a plugin must call arbitrary builder methods (`.register()`, `.provide()`,
…) itself — a rare escape hatch.
