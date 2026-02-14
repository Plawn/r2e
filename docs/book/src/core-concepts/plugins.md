# Plugins

Plugins extend R2E applications with reusable middleware, routes, and services. R2E ships with several built-in plugins and supports custom ones.

## Built-in plugins

Install plugins with `.with(plugin)` on the builder (after `build_state()`):

```rust
AppBuilder::new()
    .build_state::<AppState, _>()
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
| `Tracing` | Request tracing via `tracing` + `tower-http` |
| `ErrorHandling` | Catches panics, returns JSON 500 |
| `NormalizePath` | Trailing-slash normalization |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation |
| `SecureHeaders` | Security headers (X-Content-Type-Options, etc.) |
| `OpenApiPlugin` | OpenAPI spec + docs UI |
| `Prometheus` | Prometheus metrics at `/metrics` |

### Pre-state plugins

Some plugins need to install before `build_state()`. Use `.plugin()` instead of `.with()`:

```rust
AppBuilder::new()
    .plugin(Scheduler)    // provides CancellationToken to the bean graph
    .build_state::<AppState, _>()
    .await
    // ...
```

| Pre-state Plugin | Description |
|-----------------|-------------|
| `Scheduler` | Background task scheduling runtime |

## Plugin ordering

Plugins are installed in registration order. Some have ordering requirements:

- `NormalizePath` should be installed last (or use `should_be_last()` hint — R2E warns if plugins are added after it)
- `Tracing` should be early to capture all requests
- `ErrorHandling` should be after `Tracing` but before route registration

## Custom Tower layers

For Tower middleware that doesn't need the full plugin API, use `.with_layer()`:

```rust
use tower_http::timeout::TimeoutLayer;

AppBuilder::new()
    .build_state::<AppState, _>()
    .await
    .with_layer(TimeoutLayer::new(Duration::from_secs(30)))
    // ...
```

## Writing custom plugins

### Post-state plugins

Implement the `Plugin` trait for plugins that install after `build_state()`:

```rust
use r2e_core::plugin::Plugin;
use axum::Router;

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

### Pre-state plugins

Implement `PreStatePlugin` for plugins that need to run before `build_state()`:

```rust
use r2e_core::plugin::PreStatePlugin;

pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    fn install(self, builder: &mut AppBuilder) {
        // Store data, register deferred actions, etc.
    }
}
```

### Deferred actions

For plugins that need to perform setup after state construction, use `DeferredAction`:

```rust
use r2e_core::plugin::{DeferredAction, DeferredContext};

impl PreStatePlugin for MyPlugin {
    fn install(self, builder: &mut AppBuilder) {
        builder.defer(DeferredAction::new("my-plugin", |ctx: &mut DeferredContext| {
            ctx.add_layer(my_layer);
            ctx.on_serve(|| async { /* run when server starts */ });
            ctx.on_shutdown(|| async { /* run when server stops */ });
        }));
    }
}
```

`DeferredContext` provides:
- `add_layer()` — add a Tower layer
- `store_data()` — store data in the builder
- `on_serve()` — register a serve hook
- `on_shutdown()` — register a shutdown hook
