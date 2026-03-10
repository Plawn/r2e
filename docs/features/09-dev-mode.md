# Feature 9 — Development Mode

## Objective

Provide diagnostic endpoints and hot-reload infrastructure for development, allowing tools and scripts to detect server state and restarts.

## Core Concepts

### Dev endpoints

Two endpoints are exposed under the `/__r2e_dev/` prefix:

| Endpoint | Response | Usage |
|----------|----------|-------|
| `GET /__r2e_dev/status` | `"dev"` (plain text) | Check if the server is running in dev mode |
| `GET /__r2e_dev/ping` | JSON with `boot_time` and `status` | Detect restarts |

### Boot time

The `boot_time` is a timestamp (milliseconds since Unix epoch) captured once at process startup via `OnceLock`. When a tool detects a change in `boot_time`, it means the server has restarted.

## Usage

### 1. Enable dev mode in AppBuilder

```rust
AppBuilder::new()
    .build_state::<Services, _, _>().await
    .with(DevReload)  // Enables /__r2e_dev/* endpoints
    // ...
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### 2. Available endpoints

#### Status

```bash
curl http://localhost:3000/__r2e_dev/status
# → dev
```

Returns the string `"dev"` as plain text. Allows a script to know the server is in development mode.

#### Ping

```bash
curl http://localhost:3000/__r2e_dev/ping
# → {"boot_time":1706123456789,"status":"ok"}
```

Returns JSON with:
- `boot_time`: process startup timestamp (ms since epoch)
- `status`: always `"ok"`

## Subsecond hot-reload (recommended)

R2E supports **Subsecond hot-patching** via Dioxus 0.7. Instead of killing and restarting the server, Subsecond recompiles only the changed code as a dynamic library and patches it into the running process — typically in under 500ms.

### Configuration

1. Install the Dioxus CLI: `cargo install dioxus-cli`
2. Add the `dev-reload` feature to your app:

```toml
[features]
dev-reload = ["r2e/dev-reload"]
```

3. Structure your app with the **setup/server split** pattern:

```rust
#[derive(Clone)]
struct AppEnv {
    config: R2eConfig,
    pool: PgPool,
    event_bus: LocalEventBus,
}

async fn setup() -> AppEnv {
    // executed once, persists across hot-patches
    let config = R2eConfig::load("dev").unwrap();
    let pool = PgPool::connect("...").await.unwrap();
    let event_bus = LocalEventBus::new();
    AppEnv { config, pool, event_bus }
}

#[r2e::main]
async fn main(env: AppEnv) {
    // this body is hot-patched on every code change
    AppBuilder::new()
        .with_config(env.config)
        .provide(env.event_bus)
        .provide(env.pool)
        .with_bean::<UserService>()
        .build_state::<MyState, _, _>().await
        .with(Health)
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000").await.unwrap();
}
```

The `#[r2e::main]` macro auto-detects the parameter and generates two `#[cfg]`-gated code paths: normal execution and Subsecond hot-patching.

4. Run with: `r2e dev`

### How it works

```
Source code change
    → dx detects the change
    → recompiles ONLY the server closure as a dynamic library
    → patches it into the running process (setup state preserved)
    → ~200-500ms turnaround
```

### What goes in `setup()` vs `main()`

| `setup()` — runs once | `main(env)` — hot-patched |
|---|---|
| Database pool creation | `AppBuilder` construction |
| Config loading (`R2eConfig::load`) | Bean graph resolution (`.build_state()`) |
| Event bus creation | Controller registration |
| JWT validator setup | Plugin installation |
| SSE broadcasters, shared channels | Route definitions |
| Anything expensive or stateful | Anything you want to iterate on quickly |

**Rule of thumb:** If it holds a connection, spawns a background task, or takes more than a few ms to initialize, put it in `setup()`.

### Custom setup function

By default, the macro looks for a function named `setup`. Specify a different name via the attribute argument:

```rust
async fn my_setup() -> AppEnv { /* ... */ }

#[r2e::main(my_setup)]
async fn main(env: AppEnv) { /* ... */ }
```

### State caching

With `dev-reload`, the bean graph is cached between hot-patches. If the graph fingerprint hasn't changed, the cached state is reused for faster reloads.

Force a full rebuild with:

```rust
r2e::invalidate_state_cache();
```

### Lifecycle hooks

Startup hooks, consumer registrations, and scheduled tasks execute only on the **first** cycle. Subsequent hot-patches skip them to avoid duplicate subscriptions.

### Port conflict with Dioxus devserver

The Dioxus devserver (`dx serve`) listens on port **8080** by default. If your R2E app also binds to 8080, requests are silently intercepted. Use a different port (e.g., 3000).

### Dev headers

When `DevReload` is active, two headers are added to every response:
- `Cache-Control: no-store` — prevents browsers from caching stale responses
- `Connection: close` — avoids keep-alive connections routed to stale handlers after a hot-patch

### Legacy polling (DevReload plugin)

The `DevReload` plugin exposes `/__r2e_dev/ping` for restart detection. Still available for tools that poll for server restarts.

### Using the R2E CLI

```bash
r2e dev
r2e dev --port 8080
r2e dev --features openapi scheduler
```

This:
- Checks that `dx` CLI is installed
- Generates a `Dioxus.toml` config if absent
- Runs `dx serve --hot-patch` with the `dev-reload` feature enabled

## Best practices: environment injection in the builder

### Recommended injection order

```rust
AppBuilder::new()
    // 1. Config first — enables #[config("key")] and with_config_section
    .with_config(env.config)
    // 2. Pre-built instances from setup
    .provide(env.pool)
    .provide(env.event_bus)
    .provide(env.claims_validator)
    // 3. Typed config sections as beans
    .with_config_section::<NotificationConfig>("notification")
    // 4. Bean factories (resolved from provided + config)
    .with_bean::<UserService>()
    .with_async_bean::<CacheService>()
    .with_producer::<CreatePool>()
    // 5. Build the state
    .build_state::<Services, _, _>().await
    // 6. Post-state: plugins, controllers, hooks
    .with(Health)
    .with(Cors::permissive())
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

### Method reference

| Method | Purpose | When to use |
|--------|---------|-------------|
| `.with_config(config)` | Provide pre-loaded `R2eConfig` | Hot-reload (config loaded in setup) |
| `.load_config::<C>(profile)` | Load YAML + env overlay in one call | Simple apps without hot-reload |
| `.provide(value)` | Inject a pre-built instance | Pools, event buses, validators, shared channels |
| `.with_config_section::<T>(path)` | Deserialize config sub-tree as a bean | Typed config groups needed by multiple beans |
| `.with_bean::<T>()` | Register a sync bean factory | Services with `#[bean] impl T { fn new(...) }` |
| `.with_async_bean::<T>()` | Register an async bean factory | Services needing async init |
| `.with_producer::<T>()` | Register a producer (types you don't own) | Connection pools, external clients |

### Anti-patterns

**Don't** use `load_config` inside `main()` with hot-reload — it re-reads YAML from disk on every patch:

```rust
// Bad
#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new().load_config::<()>("dev") // reads disk every time
}

// Good
#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new().with_config(env.config) // reuses pre-loaded config
}
```

**Don't** create pools or event buses inside `main()` — they leak on every hot-patch:

```rust
// Bad: new pool on every hot-patch
let pool = PgPool::connect("...").await.unwrap();

// Good: pool from setup
AppBuilder::new().provide(env.pool)
```

**Don't** wrap the server closure in `Arc` — it breaks Subsecond's dispatch:

```rust
// Bad: Arc makes the closure pointer-sized
let server_fn = Arc::new(|env| async move { /* ... */ });

// Good: plain function or non-capturing closure
async fn __r2e_server(env: AppEnv) { /* ... */ }
```

## Production note

The `/__r2e_dev/*` endpoints must **not** be enabled in production. Don't call `.with(DevReload)` in production profile:

```rust
#[cfg(debug_assertions)]
builder = builder.with(DevReload);
```

## Validation criteria

```bash
# Status
curl http://localhost:3000/__r2e_dev/status
# → dev

# Ping
curl http://localhost:3000/__r2e_dev/ping | jq .
# → {"boot_time": 1706123456789, "status": "ok"}

# After a restart, boot_time changes
```
