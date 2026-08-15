# Custom Plugins

Plugins encapsulate reusable middleware, routes, and services. R2E supports two plugin types: post-state (`Plugin`) and pre-state (`PreStatePlugin`, which provides beans built inside `build_state()`).

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

## Pre-state plugins

Install before `build_state()` with `.plugin(plugin)`. A pre-state plugin **is
one async, fallible factory** for the beans it provides: its `build` method
runs *inside* `build_state()`, as a node of the bean graph — exactly like a
`#[bean]` constructor. No builder generics needed:

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};

pub struct MyPlugin {
    config: MyPluginConfig,
}

impl PreStatePlugin for MyPlugin {
    // `Provided` is a tuple of beans: `(A,)` for one, `(A, B)` for several, `()` for none.
    type Provided = (MyPluginConfig,);
    type Deps = ();        // no dependencies on other beans (see "Consuming application beans")
    type Config = ();

    async fn build(
        self,              // by value — owned fields move straight into the beans
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(MyPluginConfig,), PluginBuildError> {
        Ok((self.config,))
    }
}
```

`build` may await (connect to a backend, run migrations, …) and may fail: an
`Err` (any `Box<dyn Error + Send + Sync>`) aborts startup with an error naming
the plugin. Every `PreStatePlugin` must declare `type Deps` and `type Config`
— set them to `()` unless the plugin consumes an application bean (see
[Consuming application beans](#consuming-application-beans)) or reads a typed
config section (see [Typed configuration](#typed-configuration)).

## Consuming application beans

Declare the beans a plugin needs in `Deps`. They are **real edges in the bean
graph**: the framework constructs them first (in topological order) and hands
them to `build` **by value**, already resolved.

```text
  .plugin(Me)                    build_state()                       (serve)
       │                              │                                 │
       ▼                              ▼                                 ▼
  [node queued]     ─►   deps built ─► build(deps, config)  ─►   on_serve hooks
```

`Deps` can name **any** bean — provided (`.provide()`), factory-built
(`.register::<T>()`), or provided by another plugin. It is appended to the
builder's requirement list and verified against the **final** provision list
at `build_state()` — nothing is checked at the `.plugin()` call site, so the
order between `.plugin()`, `.provide()`, and `.register()` calls does not
matter. A missing dep is a guided compile error ("missing `.provide::<X>()`
or `.register::<X>()`").

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};

pub struct MetricsExporter;

impl PreStatePlugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    // `MetricsRegistry` is a factory-built bean (`.register::<MetricsRegistry>()`)
    // — fine: it is constructed before this plugin's `build` runs.
    type Deps = (MetricsRegistry,);
    type Config = ();

    async fn build(
        self,
        (registry,): (MetricsRegistry,),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ExporterHandle,), PluginBuildError> {
        let handle = ExporterHandle::connect().await?;   // async + fallible: just do it
        let h = handle.clone();
        ctx.on_serve(move |_sc| h.bind(registry));
        Ok((handle,))
    }
}
```

```rust
// `MetricsRegistry` is registered AFTER the plugin — still fine, because
// `Deps` is checked at `build_state()`, not at the `.plugin()` call site.
AppBuilder::new()
    .plugin(MetricsExporter)
    .register::<MetricsRegistry>()
    .build_state().await

// ❌ Compile error at `build_state()`: MetricsRegistry never provided —
//    guided error "missing `.provide::<MetricsRegistry>()` or
//    `.register::<MetricsRegistry>()`"
AppBuilder::new()
    .plugin(MetricsExporter)
    .build_state().await
```

Because deps arrive *before* `build` runs, a provided bean that depends on
another bean is constructed **directly** — there is no shell/fill dance.
(`r2e::Late<T>` — a `Clone`, Arc-shared, first-write-wins write-once cell —
still exists as an escape hatch for values that genuinely cannot exist until
after the whole graph is resolved; the framework's own `GraphHandle` is built
on it.)

## Typed configuration

A plugin can declare a typed config section — the same `#[derive(ConfigProperties)]`
machinery controllers use for `#[config(section)]`. The framework loads and
**validates** that section before calling `build`, and hands it over as
`Option<Self::Config>`. For raw, stringly access there is
`ctx.config_raw() -> Option<&R2eConfig>`.

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};
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
    type Config = MetricsCfg;                           // typed section
    const CONFIG_PREFIX: Option<&'static str> = Some("metrics");   // metrics.* in YAML

    async fn build(
        self,
        _deps: (),
        config: Option<MetricsCfg>,      // loaded + validated file config
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        // Precedence: builder setting (self) > file config > default.
        let endpoint = self.endpoint
            .or_else(|| config.and_then(|c| c.endpoint))
            .unwrap_or_else(|| "/metrics".into());
        ctx.add_layer(move |router| router /* mount `endpoint` */);
        Ok(())
    }
}
```

Rules for the delivered `config`:

- **`Config = ()`** (the default surface) — no config; `CONFIG_PREFIX` stays
  `None`; `build` gets `None`.
- **Presence-based (optional section).** `build` gets `Some(cfg)` only when
  `CONFIG_PREFIX` is `Some(prefix)`, config was loaded (`load_config`, or an
  `override_config` test stash consumed by it), **and** at least one key lives
  under `prefix`. No config loaded, or an absent section → `None`. This
  mirrors a controller's `Option<Section>`.
- **Validation.** A present-but-malformed section (missing required key, wrong
  type) is a **boot error** — during `build_state()` — with the same
  missing-key / type-mismatch report a controller `#[config]` mismatch
  produces, naming the plugin and section. The section is parsed even when the
  plugin is disabled (`<prefix>.enabled: false`), so it is always structurally
  validated. The precedence is: **builder setting > file config > default**.

`CONFIG_PREFIX` is an associated const with a default (`None`), so a plugin
that reads no config writes only `type Config = ();`. Because `build` runs
inside `build_state()`, config is **guaranteed loaded** there — the order
between `.plugin()` and `load_config()` does not matter.

## Enabling and disabling a plugin from config

Any plugin with a `CONFIG_PREFIX` gets an on/off switch for free: the boolean key
`<prefix>.enabled` (default **true**). Set it to `false` to turn the plugin off
without touching code:

```yaml
prometheus:
  enabled: false      # no /metrics route, no tracking layer
```

When disabled:

- **`build` still runs** — the provision list is fixed at compile time, so the
  provided beans must exist. Check `ctx.enabled()` and return a cheap,
  *disabled* variant of your beans (e.g. OpenFga returns a fail-closed
  backend; the Scheduler starts no driver task). Anything injecting the beans
  keeps compiling and running.
- **Every effect registered on the context is dropped** — layers,
  `wrap_router`, `store_data`, serve/shutdown hooks, `after_build`. Disabling
  gates the plugin's *wiring*, not its beans.
- **`pre_destroy` hooks still run** (`run_pre_destroy` registered in `setup`)
  — the beans are real and may be injected elsewhere.

Plugins with no `CONFIG_PREFIX`, and apps that never load config, are always
enabled (the flag defaults to on).

## Effects: layers, hooks, plugin data

Side effects are registered on the `PluginBuildContext` during `build` — plain
closures, no `Box`, no `DeferredAction`:

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};
use tokio_util::sync::CancellationToken;

pub struct MyPlugin;

impl PreStatePlugin for MyPlugin {
    type Provided = (CancellationToken,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancellationToken,), PluginBuildError> {
        let token = CancellationToken::new();
        let t = token.clone();

        // Add a Tower layer
        ctx.add_layer(|router| router.layer(r2e::http::Extension("my-plugin-data")));

        // Store data for later access (app.get_plugin_data::<T>())
        ctx.store_data(MyPluginHandle::new());

        // Hook into server lifecycle
        ctx.on_serve(move |_serve_ctx| {
            tracing::info!("Plugin started");
        });

        ctx.on_shutdown(move || {
            t.cancel();
            tracing::info!("Plugin shutting down");
        });

        Ok((token,))
    }
}
```

### `PluginBuildContext` methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `enabled` | `(&self) -> bool` | The `<prefix>.enabled` config gate (default `true`) |
| `graph` | `(&self) -> GraphHandle` | Cloneable handle on the **final** resolved graph (fills at the end of `build_state()`; for request-time bean lookups) |
| `config_raw` | `(&self) -> Option<&R2eConfig>` | The loaded raw config, if any |
| `add_layer` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | Add a Tower layer to the router |
| `wrap_router` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | Add an outermost transport-level router transform |
| `store_data` | `<D: Any + Send + Sync>(&mut self, D)` | Store a value keyed by type for later retrieval |
| `on_serve` | `<F: FnOnce(ServeContext) + Send + 'static>(&mut self, F)` | Run when the server starts listening |
| `on_shutdown` | `<F: FnOnce() + Send + 'static>(&mut self, F)` | Run during graceful shutdown |
| `on_shutdown_async` | `<F: FnOnce() -> Fut + Send + 'static>(&mut self, F)` | Run (and await) during graceful shutdown |
| `after_build` | `<F: FnOnce(&mut DeferredContext) + Send + 'static>(&mut self, F)` | Boot-time escape hatch with full-graph access |

Effects are buffered and applied after the graph resolves, **in plugin install
order** (`.plugin(A)` before `.plugin(B)` ⇒ A's layers apply before B's, even
if B's `build` executed first because of dependencies) — and dropped when the
plugin is disabled.

### `setup()` — rare pre-graph hook

`fn setup(&mut self, ctx: &mut PluginSetupContext)` (default no-op) runs once
at `.plugin()` time, before the graph — and possibly before config — exists.
Use it only for things other pre-state code must observe: `store_data` that
must exist even when the plugin is disabled, `run_pre_destroy::<B>()`
lifecycle registrars, or low-level `ctx.add_deferred(DeferredAction::new(..))`
actions. Everything else belongs in `build`.

## Multiple provided beans

A `PreStatePlugin` can provide **several** beans — just make `Provided` a longer
tuple and return all of them. Each element becomes its own bean in the graph:

```rust
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};
use tokio_util::sync::CancellationToken;

pub struct MyMultiPlugin;

impl PreStatePlugin for MyMultiPlugin {
    // Provides two beans: CancellationToken and MyRegistry
    type Provided = (CancellationToken, MyRegistry);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancellationToken, MyRegistry), PluginBuildError> {
        let token = CancellationToken::new();
        let registry = MyRegistry::new();

        let t = token.clone();
        ctx.on_shutdown(move || {
            t.cancel();
            tracing::info!("Shutting down");
        });

        Ok((token, registry))
    }
}
```

Both beans are then injectable by type (`#[inject] token: CancellationToken`,
`#[inject] registry: MyRegistry`).

Note that plugin-provided beans register **strictly**: an app
`.provide()`/`.register()` of the same type — or installing the same plugin
twice — is a `DuplicateBean` error at boot. In tests, pin an override
**before** `.plugin()` with `override_bean` (pinning *every* provided type
skips `build` entirely).

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
use r2e::{PreStatePlugin, PluginBuildContext, PluginBuildError};
use tokio_util::sync::CancellationToken;
use std::time::Duration;

pub struct HealthChecker {
    pub interval: Duration,
    pub url: String,
}

impl PreStatePlugin for HealthChecker {
    type Provided = (CancellationToken,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancellationToken,), PluginBuildError> {
        let token = CancellationToken::new();
        let interval = self.interval;
        let url = self.url;
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

        Ok((token,))
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
