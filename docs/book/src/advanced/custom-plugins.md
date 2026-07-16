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
    type LateDeps = ();
    type Config = ();      // no post-state dependencies (see "Consuming application beans")

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (MyPluginConfig,) {
        // `install` takes `&mut self`, so move the owned field out with `take`.
        (std::mem::take(&mut self.config),)
    }
}
```

Every `PreStatePlugin` must declare `type LateDeps` and `type Config` — set it to `()` unless the
plugin consumes an application bean after `build_state()` (see
[Consuming application beans](#consuming-application-beans)).

### Compile-time dependency checking

Plugins can declare typed dependencies via `Deps`. The compiler verifies at each `.plugin()` call site that all dependencies have already been provided:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

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

```rust
// ✅ Compiles: both deps are provided before MyPlugin
AppBuilder::new()
    .plugin(Executor)           // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)          // provides CancellationToken
    .provide(pool)              // provides DbPool
    .plugin(MyPlugin)
    .build_state().await

// ❌ Compile error: deps not yet provided
AppBuilder::new()
    .plugin(MyPlugin)           // error: DbPool not in provisions
    .plugin(Scheduler)
```

> **`Deps` are `.provide()` values only.** Because `install` runs *before* the
> bean graph is built, every type in `Deps` must be a `.provide(instance)`
> value. A `.register::<T>()`-ed (factory-built) type in `Deps` passes the
> call-site check but panics at runtime — the panic tells you to move it to
> `LateDeps`. See the next section.

## Consuming application beans

`Deps` can only name beans that already exist when the plugin installs — i.e.
things you handed to `.provide(instance)`. To consume a **factory-built** bean
(`.register::<T>()`), or a bean another plugin provides, use the second stage of
a plugin's lifecycle:

```text
  .plugin(Me)              build_state()             (serve)
       │                        │                       │
       ▼                        ▼                       ▼
    install(Deps)  ─────►  [bean graph built]  ─►  configure(LateDeps)
```

Declare the beans you need after `build_state()` in `LateDeps`, and read them in
`configure`. `LateDeps` is appended to the builder's requirement list and
verified against the **final** provision list at `build_state()` — so the
dependency may even be registered *after* your `.plugin()` call:

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredContext};

pub struct MetricsExporter;

impl PreStatePlugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    type Deps = ();
    // `MetricsRegistry` is a factory-built bean (`.register::<MetricsRegistry>()`),
    // so it cannot be a `Deps` — it does not exist yet at install time.
    type LateDeps = (MetricsRegistry,);
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (ExporterHandle,) {
        (ExporterHandle::new(),)
    }

    // Runs after `build_state()`, with the whole bean graph materialized.
    // Consumes `self`, and receives the loaded typed `Config` (here `()`).
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
```

```rust
// `MetricsRegistry` is registered AFTER the plugin — still fine, because
// `LateDeps` is checked at `build_state()`, not at the `.plugin()` call site.
AppBuilder::new()
    .plugin(MetricsExporter)
    .register::<MetricsRegistry>()
    .build_state().await
```

`configure` consumes the plugin instance (`self`) — so it can merge programmatic
builder settings with file config — and receives a borrowed copy of the plugin's
`Provided` beans, the resolved `LateDeps`, the loaded typed `Config`
(`Option<Self::Config>`, see [Typed configuration](#typed-configuration)), and a
`DeferredContext` — the same post-state surface as deferred actions (`add_layer`,
`store_data`, `on_serve`, `on_shutdown`, …). Its default is a no-op, so plugins
with `type LateDeps = ()` and `type Config = ()` never need to write it.

> **`install` takes `&mut self`.** So the instance survives into `configure`
> (which takes `self` by value). If `install` needs to move an owned field out,
> use `std::mem::take` or `.clone()`, or just leave the field for `configure`.

**Decision rule:** `Deps` = pre-built infrastructure you hand to `.provide()`;
`LateDeps` = anything else, including factory-built beans and beans other
plugins provide.

Usage:

```rust
AppBuilder::new()
    .plugin(MyPlugin { config: MyPluginConfig::default() })
    .build_state()
    .await
    // ...
```

## Typed configuration

A plugin can declare a typed config section — the same `#[derive(ConfigProperties)]`
machinery controllers use for `#[config(section)]`. The framework loads and
**validates** that section and hands it to `configure` as `Option<Self::Config>`.
`config_get` on `PluginInstallContext` stays as the stringly, low-level fallback.

```rust
use r2e::{PreStatePlugin, PluginInstallContext, DeferredContext};
use r2e::prelude::ConfigProperties;

#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct MetricsCfg {
    pub endpoint: Option<String>,   // optional keys let the builder win
    pub namespace: Option<String>,
}

pub struct Metrics { endpoint: Option<String> }  // programmatic builder setting

impl PreStatePlugin for Metrics {
    type Provided = ();
    type Deps = ();
    type LateDeps = ();
    type Config = MetricsCfg;                           // typed section
    const CONFIG_PREFIX: Option<&'static str> = Some("metrics");   // metrics.* in YAML

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) {}

    fn configure(
        self,
        _p: &(),
        (): (),
        config: Option<MetricsCfg>,      // loaded + validated file config
        ctx: &mut DeferredContext<'_>,
    ) {
        // Precedence: builder setting (self) > file config > default.
        let endpoint = self.endpoint
            .or_else(|| config.and_then(|c| c.endpoint))
            .unwrap_or_else(|| "/metrics".into());
        ctx.add_layer(Box::new(move |router| router /* mount `endpoint` */));
    }
}
```

Rules for the delivered `config`:

- **`Config = ()`** (the default surface) — no config; `CONFIG_PREFIX` stays
  `None`; `configure` gets `None`.
- **Presence-based (optional section).** `configure` gets `Some(cfg)` only when
  `CONFIG_PREFIX` is `Some(prefix)`, config was loaded (`load_config`, or an
  `override_config` test stash consumed by it), **and** at least one key lives
  under `prefix`. No config
  loaded, or an absent section → `None`. This mirrors a controller's
  `Option<Section>`.
- **Validation.** A present-but-malformed section (missing required key, wrong
  type) **panics at boot** — during `build_state()` — with the same
  missing-key / type-mismatch report a controller `#[config]` mismatch produces,
  naming the plugin and section. The precedence is: **builder setting > file
  config > default**.

`CONFIG_PREFIX` is an associated const with a default (`None`), so a plugin that
reads no config writes only `type Config = ();`. Config is delivered at
`configure` time (never at `install`) because that is the first point where
`R2eConfig` is guaranteed loaded — `load_config` always precedes `build_state()`,
whereas `.plugin()` calls may run before it.

## Enabling and disabling a plugin from config

Any plugin with a `CONFIG_PREFIX` gets an on/off switch for free: the boolean key
`<prefix>.enabled` (default **true**) controls whether the plugin's **post-state
effects** run. Set it to `false` to turn the plugin off without touching code:

```yaml
prometheus:
  enabled: false      # no /metrics route, no tracking layer
```

When disabled, the plugin's sugar (layers, `store_data`, serve/shutdown hooks),
its explicit deferred actions, **and** its `configure` are all skipped. What
does **not** change:

- **Its provided beans still exist.** The provision list is fixed at compile
  time, so a disabled plugin never removes its beans — anything injecting them
  keeps working. Disabling gates the plugin's *wiring*, not its beans.
- **`install` still runs** (it happens pre-state, before config is guaranteed
  loaded). Keep `install` cheap and put config-dependent work in `configure` or
  sugar so "disabled" is genuinely inert — this is why the built-in plugins
  defer their routes/layers to `configure`.
- **Lifecycle hooks still run.** `run_post_construct` / `run_pre_destroy` for
  provided beans fire regardless, because those beans are real and may be
  injected elsewhere.

Plugins with no `CONFIG_PREFIX`, and apps that never load config, are always
enabled (the flag defaults to on).

## Deferred actions

For plugins that need to set up infrastructure after state is built but during serve:

Call the sugar methods on `PluginInstallContext` directly — plain closures, no
`Box`, no `DeferredAction`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

impl PreStatePlugin for MyPlugin {
    type Provided = (CancellationToken,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken,) {
        let token = CancellationToken::new();
        let t = token.clone();

        // Add a Tower layer
        ctx.add_layer(|router| router.layer(r2e::http::Extension("my-plugin-data")));

        // Store data for later access
        ctx.store_data(MyPluginHandle::new());

        // Hook into server lifecycle
        ctx.on_serve(move |_serve_ctx| {
            tracing::info!("Plugin started");
        });

        ctx.on_shutdown(move || {
            t.cancel();
            tracing::info!("Plugin shutting down");
        });

        (token,)
    }
}
```

### `PluginInstallContext` post-state methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `add_layer` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | Add a Tower layer to the router |
| `wrap_router` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | Add an outermost transport-level router transform |
| `store_data` | `<D: Any + Send + Sync>(&mut self, D)` | Store a value keyed by type for later retrieval |
| `on_serve` | `<F: FnOnce(ServeContext) + Send + 'static>(&mut self, F)` | Run when the server starts listening |
| `on_shutdown` | `<F: FnOnce() + Send + 'static>(&mut self, F)` | Run during graceful shutdown |
| `on_shutdown_async` | `<F: FnOnce() -> Fut + Send + 'static>(&mut self, F)` | Run (and await) during graceful shutdown |

These calls are buffered and flushed as a single deferred action after
`build_state()`, named after the plugin type. For advanced control,
`ctx.add_deferred(DeferredAction::new(name, |dctx| { .. }))` is the low-level
escape hatch — its actions run before the buffered sugar action.

## Multiple provided beans

A `PreStatePlugin` can provide **several** beans — just make `Provided` a longer
tuple and return all of them. No builder generics, no `with_updated_types()`:

```rust
use r2e::{PreStatePlugin, PluginInstallContext};
use tokio_util::sync::CancellationToken;

pub struct MyMultiPlugin;

impl PreStatePlugin for MyMultiPlugin {
    // Provides two beans: CancellationToken and MyRegistry
    type Provided = (CancellationToken, MyRegistry);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken, MyRegistry) {
        let token = CancellationToken::new();
        let registry = MyRegistry::new();

        let t = token.clone();
        ctx.on_shutdown(move || {
            t.cancel();
            tracing::info!("Shutting down");
        });

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
use r2e::{PreStatePlugin, PluginInstallContext};
use tokio_util::sync::CancellationToken;
use std::time::Duration;

pub struct HealthChecker {
    pub interval: Duration,
    pub url: String,
}

impl PreStatePlugin for HealthChecker {
    type Provided = (CancellationToken,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken,) {
        let token = CancellationToken::new();
        let interval = self.interval;
        let url = std::mem::take(&mut self.url);
        let t = token.clone();
        let t2 = token.clone();

        // Start the checker when the server begins serving
        ctx.on_serve(move |_serve_ctx| {
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
        ctx.on_shutdown(move || {
            t.cancel();
        });

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
