# Plugins

Plugins extend R2E applications with reusable middleware, routes, and services. R2E ships with several built-in plugins and supports custom ones.

## Built-in plugins

Install plugins with `.with(plugin)` on the builder (after `build_state()`):

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
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
    .plugin(Scheduler)    // provides CancellationToken + ScheduledJobRegistry
    .build_state::<AppState, _, _>()
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
    .build_state::<AppState, _, _>()
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

### Pre-state plugins (simple path)

Implement `PreStatePlugin` for plugins that provide a single bean. No builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use r2e::type_list::TNil;

pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    type Provided = MyConfig;
    type Required = TNil;

    fn install(self, _ctx: &mut PluginInstallContext) -> MyConfig {
        MyConfig::default()
    }
}
```

### Deferred actions

For plugins that need to perform setup after state construction, use `DeferredAction` via the context:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use r2e::plugin::DeferredContext;
use r2e::type_list::TNil;

impl PreStatePlugin for MyPlugin {
    type Provided = MyToken;
    type Required = TNil;

    fn install(self, ctx: &mut PluginInstallContext) -> MyToken {
        let token = MyToken::new();
        let t = token.clone();
        ctx.add_deferred(DeferredAction::new("my-plugin", move |dctx: &mut DeferredContext| {
            dctx.add_layer(Box::new(|router| router));
            dctx.on_serve(|_tasks, _token| { /* run when server starts */ });
            dctx.on_shutdown(move || { t.cancel(); });
        }));
        token
    }
}
```

`DeferredContext` provides:
- `add_layer()` — add a Tower layer
- `store_data()` — store data in the builder
- `on_serve()` — register a serve hook
- `on_shutdown()` — register a shutdown hook

### Pre-state plugins (advanced path)

For plugins that need to provide **multiple** beans or need full builder access,
implement `RawPreStatePlugin`:

```rust
use r2e::{RawPreStatePlugin, AppBuilder, DeferredAction};
use r2e::builder::NoState;
use r2e::type_list::{TAppend, TCons, TNil};

pub struct MultiProvider;

impl RawPreStatePlugin for MultiProvider {
    type Provisions = TCons<TokenA, TCons<TokenB, TNil>>;
    type Required = TNil;

    fn install<P, R>(self, app: AppBuilder<NoState, P, R>)
        -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        app.provide(TokenA::new())
           .provide(TokenB::new())
           .with_updated_types()
    }
}
```
