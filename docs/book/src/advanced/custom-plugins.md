# Custom Plugins

Plugins encapsulate reusable middleware, routes, and services. R2E supports two plugin types: post-state (`Plugin`) and pre-state (`PreStatePlugin`).

## Post-state plugins

Install after `build_state()` with `.with(plugin)`. They receive and transform the router:

```rust
use r2e_core::plugin::Plugin;
use axum::Router;

pub struct RequestLogger;

impl<S: Clone + Send + Sync + 'static> Plugin<S> for RequestLogger {
    fn install(self, router: Router<S>) -> Router<S> {
        router.layer(axum::middleware::from_fn(|req, next| async move {
            tracing::info!("Request: {} {}", req.method(), req.uri());
            next.run(req).await
        }))
    }
}
```

Usage:

```rust
AppBuilder::new()
    .build_state::<AppState, _>()
    .await
    .with(RequestLogger)
    // ...
```

### Ordering hint

Override `should_be_last()` for plugins that must be the outermost layer:

```rust
impl<S: Clone + Send + Sync + 'static> Plugin<S> for MyPlugin {
    fn install(self, router: Router<S>) -> Router<S> { router }

    fn should_be_last(&self) -> bool {
        true  // R2E warns if plugins are added after this one
    }
}
```

## Pre-state plugins

Install before `build_state()` with `.plugin(plugin)`. They can inject values into the bean graph:

```rust
use r2e_core::plugin::PreStatePlugin;
use r2e_core::builder::AppBuilder;

pub struct MyPlugin {
    config: MyPluginConfig,
}

impl PreStatePlugin for MyPlugin {
    fn install(self, builder: &mut AppBuilder) {
        builder.provide(self.config);
    }
}
```

Usage:

```rust
AppBuilder::new()
    .plugin(MyPlugin { config: MyPluginConfig::default() })
    .build_state::<AppState, _>()
    .await
    // ...
```

## Deferred actions

For plugins that need to set up infrastructure after state is built but during serve:

```rust
use r2e_core::plugin::{DeferredAction, DeferredContext};

impl PreStatePlugin for MyPlugin {
    fn install(self, builder: &mut AppBuilder) {
        let token = CancellationToken::new();
        builder.provide(token.clone());

        builder.defer(DeferredAction::new("my-plugin", move |ctx: &mut DeferredContext| {
            // Add a Tower layer
            ctx.add_layer(my_custom_layer());

            // Store data for later access
            ctx.store_data(MyPluginHandle::new());

            // Hook into server lifecycle
            let t = token.clone();
            ctx.on_serve(move || {
                let t = t.clone();
                async move {
                    tracing::info!("Plugin started");
                }
            });

            ctx.on_shutdown(move || async move {
                tracing::info!("Plugin shutting down");
            });
        }));
    }
}
```

### DeferredContext methods

| Method | Description |
|--------|-------------|
| `add_layer(layer)` | Add a Tower layer to the router |
| `store_data(value)` | Store a value accessible later |
| `on_serve(closure)` | Run when the server starts listening |
| `on_shutdown(closure)` | Run during graceful shutdown |

## Example: Metrics plugin

```rust
pub struct MetricsPlugin {
    endpoint: String,
}

impl MetricsPlugin {
    pub fn new(endpoint: &str) -> Self {
        Self { endpoint: endpoint.to_string() }
    }
}

impl<S: Clone + Send + Sync + 'static> Plugin<S> for MetricsPlugin {
    fn install(self, router: Router<S>) -> Router<S> {
        router
            .route(&self.endpoint, get(|| async { "metrics data" }))
            .layer(/* metrics collection layer */)
    }
}
```
