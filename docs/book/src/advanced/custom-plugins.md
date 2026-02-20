# Custom Plugins

Plugins encapsulate reusable middleware, routes, and services. R2E supports two plugin types: post-state (`Plugin`) and pre-state (`PreStatePlugin` / `RawPreStatePlugin`).

## Post-state plugins

Install after `build_state()` with `.with(plugin)`. They receive and transform the `AppBuilder`:

```rust
use r2e::Plugin;
use r2e::AppBuilder;

pub struct RequestLogger;

impl Plugin for RequestLogger {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| {
            router.layer(axum::middleware::from_fn(|req, next| async move {
                tracing::info!("Request: {} {}", req.method(), req.uri());
                next.run(req).await
            }))
        })
    }
}
```

Usage:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(RequestLogger)
    // ...
```

### Ordering hint

Override `should_be_last()` for plugins that must be the outermost layer:

```rust
impl Plugin for NormalizePathPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| router.layer(NormalizePathLayer::trim_trailing_slash()))
    }

    fn should_be_last() -> bool
    where
        Self: Sized,
    {
        true // R2E warns if plugins are added after this one
    }
}
```

## Pre-state plugins (simple path)

Install before `build_state()` with `.plugin(plugin)`. Implement `PreStatePlugin` — no builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use r2e::type_list::TNil;

pub struct MyPlugin {
    config: MyPluginConfig,
}

impl PreStatePlugin for MyPlugin {
    type Provided = MyPluginConfig;
    type Required = TNil;

    fn install(self, _ctx: &mut PluginInstallContext) -> MyPluginConfig {
        self.config
    }
}
```

Usage:

```rust
AppBuilder::new()
    .plugin(MyPlugin { config: MyPluginConfig::default() })
    .build_state::<AppState, _, _>()
    .await
    // ...
```

## Deferred actions

For plugins that need to set up infrastructure after state is built but during serve:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use r2e::plugin::DeferredContext;
use r2e::type_list::TNil;
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

impl PreStatePlugin for MyPlugin {
    type Provided = CancellationToken;
    type Required = TNil;

    fn install(self, ctx: &mut PluginInstallContext) -> CancellationToken {
        let token = CancellationToken::new();

        let t = token.clone();
        ctx.add_deferred(DeferredAction::new("my-plugin", move |dctx: &mut DeferredContext| {
            // Add a Tower layer
            dctx.add_layer(Box::new(|router| router.layer(axum::Extension("my-plugin-data"))));

            // Store data for later access
            dctx.store_data(MyPluginHandle::new());

            // Hook into server lifecycle
            let t2 = t.clone();
            dctx.on_serve(move |_tasks, _cancel_token| {
                tracing::info!("Plugin started");
            });

            dctx.on_shutdown(move || {
                t2.cancel();
                tracing::info!("Plugin shutting down");
            });
        }));

        token
    }
}
```

### DeferredContext methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `add_layer` | `(&mut self, Box<dyn FnOnce(Router) -> Router + Send>)` | Add a Tower layer to the router |
| `store_data` | `<D: Any + Send + Sync>(&mut self, D)` | Store a value keyed by type for later retrieval |
| `on_serve` | `(&mut self, FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken))` | Run when the server starts listening |
| `on_shutdown` | `(&mut self, FnOnce())` | Run during graceful shutdown |

## Pre-state plugins (advanced path)

For plugins that need to provide **multiple** beans or need full builder access,
implement `RawPreStatePlugin` instead of `PreStatePlugin`:

```rust
use r2e::{RawPreStatePlugin, AppBuilder, DeferredAction};
use r2e::builder::NoState;
use r2e::type_list::{TAppend, TCons, TNil};
use tokio_util::sync::CancellationToken;

pub struct MyAdvancedPlugin;

impl RawPreStatePlugin for MyAdvancedPlugin {
    // Provides two beans: CancellationToken and MyRegistry
    type Provisions = TCons<CancellationToken, TCons<MyRegistry, TNil>>;
    type Required = TNil;

    fn install<P, R>(self, app: AppBuilder<NoState, P, R>)
        -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let token = CancellationToken::new();
        let registry = MyRegistry::new();

        app.provide(token)
           .provide(registry)
           .add_deferred(DeferredAction::new("my-advanced-plugin", |ctx| {
               ctx.on_shutdown(|| { tracing::info!("Shutting down"); });
           }))
           .with_updated_types()
    }
}
```

Usage is the same — `.plugin()` accepts both `PreStatePlugin` and `RawPreStatePlugin`:

```rust
AppBuilder::new()
    .plugin(MyAdvancedPlugin)
    .build_state::<AppState, _, _>()
    .await
    // ...
```

**When to use which:**

| | `PreStatePlugin` | `RawPreStatePlugin` |
|---|---|---|
| Provides | Single bean | Multiple beans |
| Generics | None | `<P, R>` on `install()` |
| Builder access | Via `PluginInstallContext` | Full `AppBuilder` |
| `with_updated_types()` | Not needed (handled automatically) | Required when `Required = TNil` |

## Step-by-step: Request ID plugin

A post-state plugin that adds a unique `X-Request-Id` header to every response.

```rust
use r2e::{Plugin, AppBuilder};
use axum::http::{Request, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use uuid::Uuid;

pub struct RequestId;

impl Plugin for RequestId {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| {
            router.layer(middleware::from_fn(request_id_middleware))
        })
    }
}

async fn request_id_middleware(
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "X-Request-Id",
        HeaderValue::from_str(&request_id).unwrap(),
    );
    response
}
```

Usage:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(RequestId)
    .serve("0.0.0.0:3000")
    .await;
```

## Step-by-step: Background health checker

A pre-state plugin that spawns a periodic health check task and cancels it on shutdown.

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use r2e::plugin::DeferredContext;
use r2e::type_list::TNil;
use tokio_util::sync::CancellationToken;
use std::time::Duration;

pub struct HealthChecker {
    pub interval: Duration,
    pub url: String,
}

impl PreStatePlugin for HealthChecker {
    type Provided = CancellationToken;
    type Required = TNil;

    fn install(self, ctx: &mut PluginInstallContext) -> CancellationToken {
        let token = CancellationToken::new();
        let interval = self.interval;
        let url = self.url;
        let t = token.clone();

        ctx.add_deferred(DeferredAction::new("health-checker", move |dctx: &mut DeferredContext| {
            let t2 = t.clone();

            // Start the checker when the server begins serving
            dctx.on_serve(move |_tasks, _cancel_token| {
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(interval) => {
                                match reqwest::get(&url).await {
                                    Ok(resp) => tracing::info!("Health check: {}", resp.status()),
                                    Err(e) => tracing::warn!("Health check failed: {}", e),
                                }
                            }
                            _ = t2.cancelled() => {
                                tracing::info!("Health checker stopped");
                                break;
                            }
                        }
                    }
                });
            });

            // Cancel the checker on shutdown
            dctx.on_shutdown(move || {
                t.cancel();
            });
        }));

        token
    }
}
```

Usage:

```rust
use std::time::Duration;

AppBuilder::new()
    .plugin(HealthChecker {
        interval: Duration::from_secs(30),
        url: "https://api.example.com/health".into(),
    })
    .build_state::<AppState, _, _>()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

## Available AppBuilder methods for plugin authors

Post-state plugins (`Plugin::install`) receive `AppBuilder<T>` and can call:

| Method | Description |
|--------|-------------|
| `with_layer(layer)` | Add a Tower layer (strict type bounds) |
| `with_layer_fn(\|router\| ...)` | Apply a custom router transformation (escape hatch) |
| `with_service_builder(\|router\| ...)` | Alias for `with_layer_fn` |
| `register_routes(router)` | Merge a raw `axum::Router<T>` into the app |
| `merge_router(router)` | Alias for `register_routes` |
| `on_start(\|state\| async { Ok(()) })` | Register a startup hook (runs before listening) |
| `on_stop(\|\| async { })` | Register a shutdown hook (runs after signal) |

## Example: Metrics plugin

```rust
use r2e::{Plugin, AppBuilder};
use axum::routing::get;
use axum::Router;

pub struct MetricsPlugin {
    endpoint: String,
}

impl MetricsPlugin {
    pub fn new(endpoint: &str) -> Self {
        Self { endpoint: endpoint.to_string() }
    }
}

impl Plugin for MetricsPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        let metrics_router = Router::new()
            .route(&self.endpoint, get(|| async { "metrics data" }));
        app.register_routes(metrics_router)
    }
}
```
