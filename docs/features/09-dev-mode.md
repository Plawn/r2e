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
    .build_state().await
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

3. Put the one canonical **`App` trait** implementation in `src/app.rs`.
   `lib.rs` includes it for tests and `r2e::app_main!` includes it in the binary
   tip crate while generating `main`:

```rust
// src/app.rs
pub mod controllers;
pub mod env;

use env::{setup_env, AppEnv};

pub struct MyApp;

impl App for MyApp {
    type Env = AppEnv;

    async fn setup() -> AppEnv {
        setup_env().await
    }

    async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
        // this body is hot-patched on every code change
        b.load_config::<()>()          // sole config entry; re-read from disk per patch (YAML edits apply next patch)
            .provide(env.event_bus)
            .provide(env.pool)
            .register::<UserService>()
            .build_state().await       // no type args — state inferred from the provisions
            .with(Health)
            .register_controller::<UserController>()
    }
}
```

```rust
// src/lib.rs — importable by integration tests
include!("app.rs");
```

```rust
// src/main.rs — include app.rs, generate main, and launch
r2e::app_main!(MyApp);
```

`r2e::app_main!` owns the `app.rs` inclusion and Tokio `main`; it delegates to
`r2e::launch!`, which runs `setup()` once and re-runs `build()` per hot-patch.
Without `dev-reload`, `launch!` serves normally. There is no user-written `cfg`,
crate import, or hot-reload machinery. Keep `load_config` inside `build()`:
because `build()` re-runs per patch, YAML edits are picked up on the next patch.

> **Why `launch!` is a macro, not `launch::<MyApp>()`.** Subsecond only remaps
> function symbols it attributes to the **tip crate** (the crate that owns
> `main.rs`). A generic hot-reload dispatcher monomorphised from `r2e-core` is
> *not* remapped — the jump-table lookup misses and patches never reach the
> rebuilt `build()`. `launch!` is a `macro_rules!` so its hot-reload loop —
> including the concrete `__r2e_server` function Subsecond patches — expands
> directly in your crate. `r2e::app_main!(MyApp)` emits this call for the
> conventional layout; call `launch!` yourself only for a custom `main`.

4. Run with: `r2e dev`

### How it works

```
Source code change
    → dx detects the change
    → recompiles ONLY App::build as a dynamic library
    → patches it into the running process (setup state preserved)
    → ~200-500ms turnaround
```

### Shared-source bridge: tests and real hot-reload together

Subsecond (the engine behind `dx serve --hot-patch`) **only patches the "tip"
crate — the crate that owns `main.rs`.** Changes to code in *other* crates
(including a sibling `lib.rs` in the same package, which Cargo compiles as a
separate library crate) are ignored: the running process keeps serving the
old code even though `dx` reports "Hot-patching …" and `App::build` re-runs.

R2E's scaffold resolves that Cargo/Subsecond conflict without duplicating the
declaration: `src/app.rs` is one source file compiled by two targets. The lib
copy is what `#[r2e::test(app = my_app::MyApp)]` boots. `app_main!` includes the
same source directly in the binary, so controllers, services, and `App::build`
belong to the tip crate and are genuinely patched. It does this in normal and
dev builds, avoiding feature `cfg` and crate-name-dependent imports in
`main.rs`; the `dev-reload` dependency itself remains feature-gated.

### Cold restart boundary

Controller, service, bean, and route instances are rebuilt with the router on
each patch. `App::Env` is different: the existing value deliberately survives.
Define it and its setup helpers in `src/env.rs`. `r2e dev` watches `env.rs`,
`src/env/**`, `Cargo.toml`, and `build.rs`; changing one stops and respawns `dx`
so an old allocation never crosses an incompatible struct layout. Ordinary
application files remain on the fast hot-patch path.

### What goes in `App::setup()` vs `App::build()`

| `App::setup() -> Env` — runs once | `App::build(b, env)` — hot-patched |
|---|---|
| Database pool creation | `AppBuilder` assembly |
| Event bus creation | `load_config` (re-reads YAML per patch) |
| JWT validator setup | Bean graph resolution (`.build_state()`) |
| SSE broadcasters, shared channels | Controller registration |
| Anything expensive or stateful | Plugin installation, route definitions |
| Anything you want preserved across patches | Anything you want to iterate on quickly |

**Rule of thumb:** If it holds a connection, spawns a background task, or takes more than a few ms to initialize, put it in `setup()` and thread it through `Env`.

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

### QUIC endpoint caching

When both `dev-reload` and `quic` features are enabled, the QUIC endpoint (UDP socket) is cached across hot-reload cycles — mirroring the TCP listener cache. The accept loop stops and restarts with the new router, but the socket stays bound.

**Limitation:** TLS certificate changes are not detected. If you change the cert/key files, a full process restart is required.

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

## Best practices: environment injection in `App::build`

### Recommended assembly order

```rust
async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
    b
        // 1. Config first — enables #[config("key")] and auto-registers typed
        //    sections. This is the sole config entry; because build() re-runs per
        //    patch, load_config re-reads application.yaml so YAML edits apply on
        //    the next hot-patch.
        .load_config::<RootConfig>()
        // 2. Pre-built instances from setup(), threaded through env
        .provide(env.pool)
        .provide(env.event_bus)
        .provide(env.claims_validator)
        // 3. Bean factories (resolved from provided + config)
        .register::<UserService>()
        .register::<CacheService>()
        .register::<CreatePool>()
        // 4. Build the state — no type args; the state type is the provision
        //    list materialized as an HList, inferred by the builder chain
        .build_state().await
        // 5. Post-state: plugins, controllers, hooks. Return the BootableApp —
        //    do NOT call serve here; r2e::launch! does that.
        .with(Health)
        .with(Cors::permissive())
        .register_controller::<UserController>()
}
```

There is no hand-written state struct: `.build_state().await` materializes
everything you `.provide()`d or `.register()`ed into an inferred HList state,
and controllers resolve their `#[inject]` fields from it **by type** at
`register_controller` time. A missing bean is a compile error naming the type.

> **Note:** apps with more than ~127 beans need `#![recursion_limit = "512"]`
> in each crate root (`main.rs` and `lib.rs`). `r2e doctor` warns as the bean count
> approaches the threshold.

### Method reference

| Method | Purpose | When to use |
|--------|---------|-------------|
| `.load_config::<C>()` | Load YAML + env overlay; auto-register children as beans | The one config entry — call it inside `build()`; re-reads `application.yaml` per patch, so YAML edits apply on the next hot-patch |
| `.override_config(cfg)` | Stash an in-memory `R2eConfig` for the next `load_config` | Test harness primitive — in-memory config for tests |
| `.provide(value)` | Inject a pre-built instance as a bean (by type) | Pools, event buses, validators, shared channels — from `setup()` via `env` |
| `.register::<T>()` | Register a `#[bean]` / `#[producer]` / `AsyncBean` type, resolved by the builder | Services, async-init beans, types you don't own |

### Anti-patterns

**Don't** create pools or event buses inside `App::build()` — they are rebuilt and leak on every hot-patch. Build them in `setup()` and thread them through `Env`:

```rust
// Bad: new pool on every hot-patch, inside build()
async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
    let pool = PgPool::connect("...").await.unwrap();
    b.provide(pool) /* ... */
}

// Good: pool built once in setup(), reused via env
async fn setup() -> AppEnv {
    AppEnv { pool: PgPool::connect("...").await.unwrap() }
}
async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
    b.provide(env.pool) /* ... */
}
```

**Do** keep `load_config` inside `build()`. Because `build()` re-runs per patch,
its `load_config` re-reads `application.yaml` from disk each time — deliberately,
so config file edits are picked up on the next hot-patch (a ~1 ms read beats a
dev session pinned to stale first-boot config).

> The previous `#[r2e::main] async fn main(env)` + `with_config`/`serve_with_hotreload`
> hand-wiring is gone: the hot-patch loop lives in the `r2e::launch!` macro
> (which expands into your crate so Subsecond can patch it). Express the split
> with `App::setup` / `App::build` instead.

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
