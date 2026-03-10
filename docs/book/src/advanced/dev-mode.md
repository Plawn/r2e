# Dev Mode

R2E provides development-mode endpoints for hot-reload detection and diagnostics.

## Enabling dev mode

Add the `DevReload` plugin:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(DevReload)
    // ...
```

## Dev endpoints

### Status

```
GET /__r2e_dev/status → "dev"
```

Returns plain text `"dev"`. Use to check if the server is running in development mode.

### Ping

```
GET /__r2e_dev/ping → {"boot_time": 1234567890123, "status": "ok"}
```

Returns the server's boot timestamp (milliseconds since epoch). Use to detect server restarts.

## Subsecond hot-reload (recommended)

R2E supports **Subsecond hot-patching** via Dioxus 0.7. Instead of killing and restarting the server, Subsecond recompiles only the changed code as a dynamic library and patches it into the running process — typically in under 500ms.

### Setup

1. Install the Dioxus CLI: `cargo install dioxus-cli`
2. Add `dev-reload` feature to your app:

```toml
[features]
dev-reload = ["r2e/dev-reload"]
```

3. Structure your app with the **setup/server split** pattern:

```rust
#[derive(Clone)]
struct AppEnv {
    pool: PgPool,
    config: R2eConfig,
    event_bus: LocalEventBus,
}

async fn setup() -> AppEnv {
    // runs ONCE, persists across hot-patches
    let config = R2eConfig::load().unwrap();
    let pool = PgPool::connect("...").await.unwrap();
    let event_bus = LocalEventBus::new();
    AppEnv { pool, config, event_bus }
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
        .with(Cors::permissive())
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000").await.unwrap();
}
```

The `#[r2e::main]` macro auto-detects the parameter and generates two `#[cfg]`-gated code paths: normal execution and Subsecond hot-patching.

4. Run with: `r2e dev`

### How it works

```
Source code change
    → dx detects change
    → recompiles ONLY the server closure as a dynamic library
    → patches it into the running process (setup state preserved)
    → ~200-500ms turnaround
```

### What goes in `setup()` vs `main()`

The split between setup and server is critical for correct hot-reload behavior:

| `setup()` — runs once | `main(env)` — hot-patched |
|---|---|
| Database pool creation | `AppBuilder` construction |
| Config loading (`R2eConfig::load`) | Bean graph resolution (`.build_state()`) |
| Event bus creation | Controller registration |
| JWT validator setup | Plugin installation |
| SSE broadcasters, shared channels | Route definitions |
| Anything expensive or stateful | Anything you want to iterate on quickly |

**Rule of thumb:** If it holds a connection, spawns a background task, or takes more than a few ms to initialize, put it in `setup()`.

### Specifying a custom setup function

By default, the macro looks for a function named `setup`. Use the attribute argument to specify a different name:

```rust
async fn my_custom_setup() -> AppEnv { /* ... */ }

#[r2e::main(my_custom_setup)]
async fn main(env: AppEnv) {
    // ...
}
```

### State caching

When the `dev-reload` feature is active, the bean graph is cached between hot-patches. If the graph fingerprint hasn't changed (no bean constructors modified), the cached state is reused — making reloads even faster.

To force a full state rebuild (e.g., after changing a bean constructor), call:

```rust
r2e::invalidate_state_cache();
```

### Lifecycle hooks and hot-reload

Startup hooks (`on_start`), consumer registrations (`#[consumer]`), and scheduled tasks (`#[scheduled]`) are only executed on the **first** cycle. Subsequent hot-patches skip them to avoid duplicate subscriptions or double-started tasks.

### Port conflict with Dioxus devserver

The Dioxus devserver (`dx serve`) listens on port **8080** by default. If your R2E app also binds to 8080, requests will be silently intercepted and never reach your app. Use a different port:

```rust
// Good
.serve("0.0.0.0:3000").await.unwrap();

// Bad — conflicts with dx devserver
.serve("0.0.0.0:8080").await.unwrap();
```

### Dev headers

When `DevReload` is active, R2E adds two headers to every response:

- `Cache-Control: no-store` — prevents browsers from caching stale API responses
- `Connection: close` — forces the browser to close TCP connections after each response, avoiding stale keep-alive connections routed to old handler tasks after a hot-patch

## Legacy polling (DevReload plugin)

The `DevReload` plugin exposes `/__r2e_dev/ping` for restart detection. This is still available for tools that poll for server restarts (e.g., browser scripts that detect `boot_time` changes and auto-refresh).

## Using `r2e dev`

The CLI starts the Subsecond hot-reload dev server:

```bash
r2e dev
r2e dev --port 8080
r2e dev --features openapi scheduler
```

This:
- Checks that `dx` CLI is installed (prints instructions if missing)
- Generates a `Dioxus.toml` config if absent
- Runs `dx serve --hot-patch` with the `dev-reload` feature enabled

## Best practices: environment injection in the builder

When using hot-reload, the `AppEnv` struct carries resources from `setup()` into the hot-patched `main()`. Here are the recommended patterns for injecting them into the `AppBuilder`.

### Use `with_config()` for pre-loaded configuration

```rust
// Best: config loaded once in setup, reused across hot-patches
AppBuilder::new()
    .with_config(env.config)
```

This stores the raw config in the builder **and** provides `R2eConfig` in the bean registry. Beans using `#[config("key")]` and `#[config_section(prefix = "...")]` both work.

### Use `provide()` for pre-built instances

```rust
AppBuilder::new()
    .with_config(env.config)
    .provide(env.pool)                    // SqlitePool
    .provide(env.event_bus)               // LocalEventBus
    .provide(env.claims_validator)        // Arc<JwtClaimsValidator>
```

`provide()` injects a pre-built value directly into the bean graph. Use it for anything constructed in `setup()` that beans or controllers depend on.

### Use `#[config_section(prefix = "...")]` for typed config sub-trees

Config sections are injected directly in beans and producers using `#[config_section(prefix = "...")]`:

```rust
#[bean]
impl SearchService {
    fn new(
        #[config_section(prefix = "matching")] matching: MatchingConfig,
        other_dep: OtherDep,
    ) -> Self { ... }
}
```

This requires `#[derive(ConfigProperties, Clone)]` on the config struct. Field-level `#[config(default)]`, `#[config(env)]`, etc. are respected.

### Use `with_bean` / `with_async_bean` / `with_producer` for constructed services

```rust
AppBuilder::new()
    .with_config(env.config)
    .provide(env.pool)
    .provide(env.event_bus)
    .with_bean::<UserService>()              // sync constructor
    .with_async_bean::<CacheService>()       // async constructor
    .with_producer::<CreateNotifier>()       // factory for types you don't own
    .build_state::<Services, _, _>().await
```

The bean graph resolves dependencies automatically. If `UserService::new` takes `SqlitePool` and `LocalEventBus`, they are pulled from what you `provide()`d.

### Complete recommended pattern

```rust
#[derive(Clone)]
struct AppEnv {
    config: R2eConfig,
    pool: PgPool,
    event_bus: LocalEventBus,
    claims_validator: Arc<JwtClaimsValidator>,
}

async fn setup() -> AppEnv {
    let config = R2eConfig::load().unwrap();
    let pool = PgPool::connect(&config.get::<String>("database.url").unwrap())
        .await.unwrap();
    let event_bus = LocalEventBus::new();
    let claims_validator = Arc::new(JwtClaimsValidator::new(/* ... */));
    AppEnv { config, pool, event_bus, claims_validator }
}

#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new()
        // 1. Config first — enables #[config("key")] and #[config_section(prefix = "...")]
        .with_config(env.config)
        // 2. Provide pre-built instances from setup
        .provide(env.pool)
        .provide(env.event_bus)
        .provide(env.claims_validator)
        // 3. Bean factories (resolved from provided + config)
        .with_bean::<UserService>()
        .with_async_bean::<CacheService>()
        // 5. Build the state
        .build_state::<Services, _, _>().await
        // 6. Post-state plugins and controllers
        .with(Health)
        .with(Cors::permissive())
        .with(ErrorHandling)
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000").await.unwrap();
}
```

### Anti-patterns

**Don't** call `load_config` in `main()` when using hot-reload — it re-reads YAML from disk on every patch:

```rust
// Bad: re-reads config on every hot-patch
#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new()
        .load_config::<()>()  // reads disk every time
        // ...
}

// Good: config loaded once in setup, passed via env
#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new()
        .with_config(env.config)   // reuses pre-loaded config
        // ...
}
```

**Don't** create connection pools or event buses inside `main()` — they leak on every hot-patch:

```rust
// Bad: new pool on every hot-patch (leaks connections)
#[r2e::main]
async fn main(env: AppEnv) {
    let pool = PgPool::connect("...").await.unwrap();
    AppBuilder::new().provide(pool)
    // ...
}

// Good: pool created once in setup
#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new().provide(env.pool)
    // ...
}
```

**Don't** wrap the server closure in `Arc` — it breaks Subsecond's hot-patching dispatch:

```rust
// Bad: Arc makes the closure pointer-sized, breaking hot-patch
let server_fn = Arc::new(|env| async move { /* ... */ });

// Good: use a plain function or non-capturing closure
async fn __r2e_server(env: AppEnv) { /* ... */ }
```

## Production note

Do **not** enable `DevReload` in production. The dev endpoints are informational only but expose internal details (boot time) that shouldn't be public.

```rust
#[cfg(debug_assertions)]
builder = builder.with(DevReload);
```
