# Custom Plugins

Plugins encapsulate reusable middleware, routes, and services. R2E supports two plugin types: post-state (`Plugin`) and pre-state (`PreStatePlugin`, which provides beans before `build_state()`).

## Post-state plugins

Install after `build_state()` with `.with(plugin)`. They receive and transform the `AppBuilder`:

```rust
use r2e::Plugin;
use r2e::AppBuilder;

pub struct RequestLogger;

impl Plugin for RequestLogger {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| {
            router.layer(r2e::http::middleware::from_fn(|req, next| async move {
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
    .build_state()
    .await
    .with(RequestLogger)
    // ...
```

### Ordering hint

Override `should_be_last()` for plugins that must be the outermost layer:

```rust
impl Plugin for CompressionPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| router.layer(CompressionLayer::new()))
    }

    fn should_be_last() -> bool
    where
        Self: Sized,
    {
        true // R2E warns if plugins are added after this one
    }
}
```

Note that layers added via `Router::layer` run *after* routing — they cannot
rewrite the request URI in a way that changes which route matches. R2E's
built-in `NormalizePath` plugin is instead applied at build time as a
pre-routing rewrite wrapping the whole router, which is why it has no
ordering constraint.

## Pre-state plugins (simple path)

Install before `build_state()` with `.plugin(plugin)`. Implement `PreStatePlugin` — no builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};

pub struct MyPlugin {
    config: MyPluginConfig,
}

impl PreStatePlugin for MyPlugin {
    // `Provided` is a tuple of beans: `(A,)` for one, `(A, B)` for several, `()` for none.
    type Provided = (MyPluginConfig,);
    type Deps = ();

    fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (MyPluginConfig,) {
        (self.config,)
    }
}
```

### Compile-time dependency checking

Plugins can declare typed dependencies via `Deps`. The compiler verifies at each `.plugin()` call site that all dependencies have already been provided:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

impl PreStatePlugin for MyPlugin {
    type Provided = (MyService,);
    type Deps = (DbPool, CancellationToken);

    fn install(self, (pool, token): (DbPool, CancellationToken), _ctx: &mut PluginInstallContext<'_>) -> (MyService,) {
        (MyService::new(pool, token),)
    }
}
```

```rust
// ✅ Compiles: both deps are provided before MyPlugin
AppBuilder::new()
    .plugin(Scheduler)          // provides CancellationToken
    .provide(pool)              // provides DbPool
    .plugin(MyPlugin)
    .build_state().await

// ❌ Compile error: deps not yet provided
AppBuilder::new()
    .plugin(MyPlugin)           // error: DbPool not in provisions
    .plugin(Scheduler)
```

Usage:

```rust
AppBuilder::new()
    .plugin(MyPlugin { config: MyPluginConfig::default() })
    .build_state()
    .await
    // ...
```

## Deferred actions

For plugins that need to set up infrastructure after state is built but during serve:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use r2e::plugin::DeferredContext;
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

impl PreStatePlugin for MyPlugin {
    type Provided = (CancellationToken,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken,) {
        let token = CancellationToken::new();

        let t = token.clone();
        ctx.add_deferred(DeferredAction::new("my-plugin", move |dctx: &mut DeferredContext| {
            // Add a Tower layer
            dctx.add_layer(Box::new(|router| router.layer(r2e::http::Extension("my-plugin-data"))));

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

        (token,)
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

## Multiple provided beans

A `PreStatePlugin` can provide **several** beans — just make `Provided` a longer
tuple and return all of them. No builder generics, no `with_updated_types()`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredAction};
use tokio_util::sync::CancellationToken;

pub struct MyMultiPlugin;

impl PreStatePlugin for MyMultiPlugin {
    // Provides two beans: CancellationToken and MyRegistry
    type Provided = (CancellationToken, MyRegistry);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken, MyRegistry) {
        let token = CancellationToken::new();
        let registry = MyRegistry::new();

        let t = token.clone();
        ctx.add_deferred(DeferredAction::new("my-multi-plugin", move |dctx| {
            dctx.on_shutdown(move || {
                t.cancel();
                tracing::info!("Shutting down");
            });
        }));

        (token, registry)
    }
}
```

Both beans are then injectable by type (`#[inject] token: CancellationToken`,
`#[inject] registry: MyRegistry`).

### Escape hatch: `RawPreStatePlugin`

`RawPreStatePlugin` is the internal, HList-based trait that `.plugin()` actually
dispatches on; every `PreStatePlugin` gets one for free via a blanket impl.
Because `PreStatePlugin` now covers multiple provided beans, the **only** reason
to hand-write a `RawPreStatePlugin` is to call arbitrary builder methods
(`.register()`, `.provide()`, `.when()`, …) during install. It is `#[doc(hidden)]`
and almost never needed — reach for it only when a plugin genuinely has to drive
the builder itself.

## Step-by-step: Request ID plugin

A post-state plugin that adds a unique `X-Request-Id` header to every response.

```rust
use r2e::prelude::*; // Plugin, AppBuilder, Request, Next, Response
use r2e::http::header::HeaderValue;
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
    request: Request<Body>,
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
    .build_state()
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
use tokio_util::sync::CancellationToken;
use std::time::Duration;

pub struct HealthChecker {
    pub interval: Duration,
    pub url: String,
}

impl PreStatePlugin for HealthChecker {
    type Provided = (CancellationToken,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken,) {
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

        (token,)
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
    .build_state()
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
| `register_routes(router)` | Merge a `Router<T>` into the app |
| `merge_router(router)` | Alias for `register_routes` |
| `on_start(\|state\| async { Ok(()) })` | Register a startup hook (runs before listening) |
| `on_stop(\|\| async { })` | Register a shutdown hook (runs after signal) |

## Example: Metrics plugin

```rust
use r2e::prelude::*; // Plugin, AppBuilder, Router
use r2e::http::routing::get;

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
