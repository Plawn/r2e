# Custom Plugins

Plugins encapsulate reusable middleware, routes, controllers, and services.
There is exactly **one** plugin trait — `Plugin` — one install call —
`.plugin(p)`, always **before** `build_state()` — and one factory method,
`build`, which runs *inside* `build_state()` as a node of the bean graph.

## Anatomy of a plugin

A plugin **is one async, fallible factory** for the beans it provides: `build`
runs inside `build_state()`, exactly like a `#[bean]` constructor. No builder
generics needed:

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

pub struct MyPlugin {
    config: MyPluginConfig,
}

impl Plugin for MyPlugin {
    // `Provided` is a tuple of beans: `(A,)` for one, `(A, B)` for several, `()` for none.
    type Provided = (MyPluginConfig,);
    type Deps = ();          // no dependencies on other beans (see "Consuming application beans")
    type Config = ();        // no typed config section (see "Typed configuration")
    type Controllers = ();   // no controllers shipped (see "Shipping controllers")

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

Usage:

```rust
AppBuilder::new()
    .plugin(MyPlugin { config: MyPluginConfig::default() })
    .build_state()
    .await
    // ...
```

`build` may await (connect to a backend, run migrations, …) and may fail: an
`Err` (any `Box<dyn Error + Send + Sync>`) aborts startup with an error naming
the plugin. All four associated types are mandatory — stable Rust has no
associated-type defaults — so write `()` for the ones you do not use.

## Effect stages: Graph, Routes, Finalize

A plugin never touches the router directly. It registers **effects** on the
`PluginBuildContext`, and each effect belongs to a stage:

| Stage | Registered with | Applied |
|---|---|---|
| **Graph** | `add_layer`, `store_data`, `on_serve`, `after_build` | right after the bean graph resolves |
| **Routes** | `after_routes` | after **every** controller (app, module, plugin) is registered |
| **Finalize** | `wrap_router` | outermost, after every HTTP layer |

Within a stage, effects apply in **install order** (a later layer/wrap ends up
*outside* an earlier one); builds themselves run in dependency order, so the two
orders are independent. Nothing needs to be installed "last" any more: whatever
used to need the complete route table registers a Routes effect, and whatever
used to need to be outermost registers a Finalize effect.

Note that layers added via `Router::layer` run *after* routing — they cannot
rewrite the request URI in a way that changes which route matches. R2E's
built-in `NormalizePath` plugin is instead applied at build time as a
pre-routing rewrite wrapping the whole router, which is why it has no
ordering constraint.

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
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

pub struct MetricsExporter;

impl Plugin for MetricsExporter {
    type Provided = (ExporterHandle,);
    // `MetricsRegistry` is a factory-built bean (`.register::<MetricsRegistry>()`)
    // — fine: it is constructed before this plugin's `build` runs.
    type Deps = (MetricsRegistry,);
    type Config = ();
    type Controllers = ();

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
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::prelude::ConfigProperties;

#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct MetricsCfg {
    pub endpoint: Option<String>,   // optional keys let the builder win
    pub namespace: Option<String>,
}

pub struct Metrics { endpoint: Option<String> }  // programmatic builder setting

impl Plugin for Metrics {
    type Provided = ();
    type Deps = ();
    type Config = MetricsCfg;                           // typed section
    type Controllers = ();
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
- **All three surface stages are dropped** — Graph (`add_layer`, `store_data`,
  `on_serve`, `after_build`), Routes (`after_routes`) and Finalize
  (`wrap_router`). Disabling gates the plugin's *wiring*, not its beans.
- **Cleanup effects still run** — `on_shutdown` and `on_shutdown_async`.
  `build` ran, so whatever it constructed still has to be released; dropping
  its disposal would leak exactly what a disabled plugin built. Keep those
  hooks to disposal only.
- **`pre_destroy` hooks still run** (`run_pre_destroy` registered in `setup`)
  — the beans are real and may be injected elsewhere.

Making the beans *inert* is the plugin's job: check `ctx.enabled()` before any
process-global side effect inside `build` itself, not just around the effects
(Prometheus, for example, returns early so the global metrics recorder is never
installed).

Plugins with no `CONFIG_PREFIX`, and apps that never load config, are always
enabled (the flag defaults to on).

## Effects: layers, hooks, plugin data

Side effects are registered on the `PluginBuildContext` during `build` — plain
closures, no `Box`, no `DeferredAction`:

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::rt::CancelToken;

pub struct MyPlugin;

impl Plugin for MyPlugin {
    type Provided = (CancelToken,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancelToken,), PluginBuildError> {
        let token = CancelToken::new();
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
| `graph` | `(&self) -> GraphHandle` | Cloneable **weak** handle on the **final** resolved graph (fills at the end of a successful `build_state()`; reads `Some` for the app's whole life *and* for any tracked task that outlives it, since the router, every tracked task and the serving scope each hold the graph independently — it reads `None` only once the last of those owners is gone) |
| `config_raw` | `(&self) -> Option<&R2eConfig>` | The loaded raw config, if any |
| `add_layer` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | **Graph** — add a Tower layer to the router |
| `store_data` | `<D: Any + Send + Sync>(&mut self, D)` | **Graph** — store a value keyed by type for later retrieval |
| `on_serve` | `<F: FnOnce(ServeContext) + Send + 'static>(&mut self, F)` | **Graph** — run when the server starts listening |
| `after_build` | `<F: FnOnce(&mut DeferredContext) + Send + 'static>(&mut self, F)` | **Graph** — boot-time escape hatch with full-graph access |
| `after_routes` | `<F: FnOnce(&mut RoutesContext) + Send + 'static>(&mut self, F)` | **Routes** — runs after every controller is registered |
| `wrap_router` | `<F: FnOnce(Router) -> Router + Send + 'static>(&mut self, F)` | **Finalize** — outermost transport-level router transform |
| `on_shutdown` | `<F: FnOnce() + Send + 'static>(&mut self, F)` | cleanup — run during graceful shutdown |
| `on_shutdown_async` | `<F: FnOnce() -> Fut + Send + 'static>(&mut self, F)` | cleanup — run (and await) during graceful shutdown |

Effects are buffered and applied per stage, **in plugin install order** within a
stage (`.plugin(A)` before `.plugin(B)` ⇒ A's layers apply inside B's, even
if B's `build` executed first because of dependencies) — and every surface stage
is dropped when the plugin is disabled (shutdown hooks are not; see above).

### The Routes stage and the route registry

`after_routes` hands you a `RoutesContext`, the only place with the **complete**
route table:

```rust
ctx.after_routes(move |routes| {
    for route in routes.routes() {          // &[RouteInfo] — every registered route
        tracing::debug!("{} {}", route.method, route.path);
    }
    routes.register_routes(my_router);      // mount your own routes
});
```

Because the stage runs after *all* controllers are registered — app,
feature-module and plugin ones alike — a Routes effect is install-order
independent. This is how `OpenApiPlugin` builds its spec and mounts
`/openapi.json` without having to be installed last.

When an effect needs one of the plugin's own provided beans, resolve it from the
graph at apply time — `ctx.after_build(|dctx| { let x =
dctx.bean_context().try_get::<X>(); … })` — instead of capturing the value
`build` just made. A test that pins only *some* of the provisions with
`override_bean` still runs `build`, and the graph then exposes the pinned bean
while your captured one is invisible to everyone else.

### `setup()` — rare pre-graph hook

`fn setup(&mut self, ctx: &mut PluginSetupContext)` (default no-op) runs once
at `.plugin()` time, before the graph — and possibly before config — exists.
Use it only for things other builder-phase code must observe: `store_data` that
must exist even when the plugin is disabled, `run_pre_destroy::<B>()`
lifecycle registrars, or low-level `ctx.add_deferred(DeferredAction::new(..))`
actions. Everything else belongs in `build`.

Setup actions are **never** gated on `<prefix>.enabled` — which is why the setup
context carries no surface-effect sugar: there is no `add_layer`,
`wrap_router`, `on_serve` or shutdown hook on `PluginSetupContext`, and reaching
for one is a compile error. So a disabled plugin cannot mount a route *by
accident*.

`add_deferred` is the deliberate hole: it hands you a full `DeferredContext`,
which the framework cannot gate for you, so a route mounted from there is
mounted whether the plugin is enabled or not. That is the price of the escape
hatch — if what you register should disappear under `enabled = false`, register
it from `build` instead.

## Multiple provided beans

A `Plugin` can provide **several** beans — just make `Provided` a longer
tuple and return all of them. Each element becomes its own bean in the graph:

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::rt::CancelToken;

pub struct MyMultiPlugin;

impl Plugin for MyMultiPlugin {
    // Provides two beans: CancelToken and MyRegistry
    type Provided = (CancelToken, MyRegistry);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancelToken, MyRegistry), PluginBuildError> {
        let token = CancelToken::new();
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

Both beans are then injectable by type (`#[inject] token: CancelToken`,
`#[inject] registry: MyRegistry`).

Note that plugin-provided beans register **strictly**: an app
`.provide()`/`.register()` of the same type — or installing the same plugin
twice — is a `DuplicateBean` error at boot. In tests, pin an override
**before** `.plugin()` with `override_bean`: the pin wins for that type and
`build` still runs, so the plugin's routes, layers and hooks stay in place
(they are not beans, and no pin can replace them). A plugin whose `build` is
pure bean construction *and* expensive — a connection, a container, key
generation — can opt out with `const SKIP_BUILD_WHEN_ALL_PINNED: bool = true;`,
and then pinning *every* provided type skips `build` entirely. To silence a
plugin that carries effects, disable it with `<prefix>.enabled = false`.

### Escape hatch: `PluginInstall`

`PluginInstall` is the internal, HList-based trait that `.plugin()` actually
dispatches on; every `Plugin` gets one for free via a blanket impl.
Because `Plugin` now covers multiple provided beans, the **only** reason
to hand-write a `PluginInstall` is to call arbitrary builder methods
(`.register()`, `.provide()`, `.when()`, …) during install. It is `#[doc(hidden)]`
and almost never needed — reach for it only when a plugin genuinely has to drive
the builder itself.

## Step-by-step: Request ID plugin

A plugin that adds a unique `X-Request-Id` header to every response — one Graph
effect, no beans.

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::prelude::*; // Request, Next, Response, middleware
use r2e::http::header::HeaderValue;
use uuid::Uuid;

pub struct RequestId;

impl Plugin for RequestId {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        ctx.add_layer(|router| router.layer(middleware::from_fn(request_id_middleware)));
        Ok(())
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
    .plugin(RequestId)
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

## Shipping controllers from a plugin

Instead of hand-assembling a `Router`, a plugin can declare `#[controller]`
types in `Controllers`. They are written exactly like application controllers —
guards, `#[roles]`, extractors and OpenAPI metadata all work — and they may
`#[inject]` the plugin's own provided beans as well as any application bean:

```rust
use r2e::prelude::*;
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

#[controller(path = "/metrics")]
pub struct MetricsController {
    #[inject] registry: MetricsRegistry,   // the plugin's own provided bean
}

#[routes]
impl MetricsController {
    #[get("/")]
    async fn scrape(&self) -> String { self.registry.render() }
}

pub struct Metrics;

impl Plugin for Metrics {
    type Provided = (MetricsRegistry,);
    type Deps = ();
    type Config = ();
    type Controllers = (MetricsController,);

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(MetricsRegistry,), PluginBuildError> {
        Ok((MetricsRegistry::new(),))
    }
}
```

Plugin controllers are registered by `build_state()` right after the graph
resolves, so their routes are part of the route registry every `after_routes`
effect sees. Their `#[inject]` fields ride along in the builder's requirement
list and are checked against the **final** provision list at `build_state()`,
exactly like `Deps` — a bean the controller needs but nobody provides is a
compile error naming the plugin.

## Step-by-step: Background health checker

A plugin that spawns a periodic health check task and cancels it on shutdown.

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::rt::CancelToken;
use std::time::Duration;

pub struct HealthChecker {
    pub interval: Duration,
    pub url: String,
}

impl Plugin for HealthChecker {
    type Provided = (CancelToken,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(CancelToken,), PluginBuildError> {
        let token = CancelToken::new();
        let interval = self.interval;
        let url = self.url;
        let t = token.clone();
        let t2 = token.clone();

        // Start the checker when the server begins serving
        ctx.on_serve(move |_serve_ctx| {
            r2e::rt::spawn(async move {
                loop {
                    r2e::rt::select! {
                        _ = r2e::rt::sleep(interval) => {
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

## Available AppBuilder methods (application side)

These are methods **the application** calls on the built app (after
`build_state()`); a plugin reaches the same surface through its effects rather
than by receiving the builder:

| Method | Plugin equivalent |
|--------|-------------------|
| `with_layer(layer)` | `ctx.add_layer(\|r\| r.layer(layer))` |
| `with_layer_fn(\|router\| ...)` | `ctx.add_layer(f)` |
| `register_routes(router)` / `merge_router(router)` | `ctx.after_routes(\|routes\| routes.register_routes(r))`, or `type Controllers` |
| `on_start(\|state\| async { Ok(()) })` | `ctx.on_serve(f)` |
| `on_stop(\|\| async { })` | `ctx.on_shutdown(f)` / `ctx.on_shutdown_async(f)` |

## Example: Metrics plugin

```rust
use r2e::{Plugin, PluginBuildContext, PluginBuildError};
use r2e::http::{routing::get, Router};

pub struct MetricsPlugin {
    endpoint: String,
}

impl MetricsPlugin {
    pub fn new(endpoint: &str) -> Self {
        Self { endpoint: endpoint.to_string() }
    }
}

impl Plugin for MetricsPlugin {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        let endpoint = self.endpoint;
        ctx.after_routes(move |routes| {
            routes.register_routes(
                Router::new().route(&endpoint, get(|| async { "metrics data" })),
            );
        });
        Ok(())
    }
}
```
