# Dev Mode

R2E supports **Subsecond hot-patching** via Dioxus 0.7 for instant code reloading during development. Instead of killing and restarting the server, Subsecond recompiles only the changed code as a dynamic library and patches it into the running process — typically in under 500ms.

## Setup

1. Install the Dioxus CLI: `cargo install dioxus-cli`
2. Add `dev-reload` feature to your app:

```toml
[features]
dev-reload = ["r2e/dev-reload"]
```

3. Put the canonical **`App` trait** implementation in `src/app.rs`. `lib.rs`
   compiles it for tests and `app_main!` compiles it in the binary tip crate:

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
            .build_state().await
            .with(Health)
            .with(Cors::permissive())
            .with(DevReload)
            .register_controller::<UserController>()
    }
}
```

```rust
// src/lib.rs — integration tests
include!("app.rs");
```

```rust
// src/main.rs — app.rs becomes binary tip-crate code
r2e::app_main!(MyApp);
```

`r2e::app_main!` includes `src/app.rs` in the binary, generates the Tokio
`main`, and delegates to `r2e::launch!`. Under `dev-reload`, `launch!` runs
`setup()` once and re-runs `build()` per hot-patch. It is a macro (not
`launch::<MyApp>()`) because Subsecond only patches functions in the *tip crate*
that owns `main.rs`; its hot-reload loop — including the concrete function
Subsecond remaps — must expand directly into your crate. The unconditional
include performed by `app_main!` makes the app, controllers, and services
tip-crate code in both dev and production, with no user-written `cfg` or
crate-name import. Tests use the library inclusion of the same source.

Keep the persistent `AppEnv` and its setup helper in `src/env.rs`. Changes to
that cold file, `src/env/**`, `Cargo.toml`, or `build.rs` make `r2e dev` perform
a safe full process restart. Other application changes stay on the hot-patch
path.

### What goes in `App::setup()` vs `App::build()`

This split is critical for correct hot-reload behavior:

| `App::setup() -> Env` — runs once | `App::build(b, env)` — hot-patched |
|---|---|
| Database pool creation | `AppBuilder` assembly |
| Event bus creation | `load_config` (re-reads YAML per patch) |
| JWT validator setup | Bean graph resolution (`.build_state()`) |
| SSE broadcasters, shared channels | Controller registration |
| Anything expensive or stateful | Plugin installation, route definitions |
| Anything you want preserved across patches | Anything you want to iterate on quickly |

**Rule of thumb:** If it holds a connection, spawns a background task, or takes more than a few ms to initialize, put it in `setup()` and thread it through `Env`.

4. Run with: `r2e dev`

```bash
r2e dev
r2e dev --port 8080
r2e dev --features openapi scheduler
```

## How it works

```
Source code change
    → dx detects change
    → recompiles the changed tip-crate application code as a dynamic library
    → patches it into the running process (setup state preserved)
    → ~200-500ms turnaround
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

The Dioxus devserver (`dx serve`) listens on port **8080** by default. If your R2E app also binds to 8080, requests will be silently intercepted and never reach your app. Use a different port via config (`launch` reads `server.port`):

```yaml
# application.yaml — keep your app off 8080 during dev
server:
  port: 3000
```

## Anti-patterns

**Don't** create connection pools or event buses inside `App::build()` — they are rebuilt and leak on every hot-patch. Build them in `setup()` and thread them through `Env`:

```rust
// Bad: new pool on every hot-patch (leaks connections)
async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
    let pool = PgPool::connect("...").await.unwrap();
    b.provide(pool) /* ... */
}

// Good: pool created once in setup(), reused via env
async fn setup() -> AppEnv {
    AppEnv { pool: PgPool::connect("...").await.unwrap() }
}
async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
    b.provide(env.pool) /* ... */
}
```

**Do** keep `load_config` inside `build()`. Because `build()` re-runs per patch,
its `load_config` re-reads `application.yaml` from disk each time — deliberately,
so config file edits are picked up on the next hot-patch (the ~1 ms read beats a
dev session pinned to stale first-boot config). The old
`#[r2e::main] async fn main(env)` + `with_config` hand-wiring is gone —
express the split with `App::setup` / `App::build` and launch via `r2e::launch!`.

## DevReload plugin

The `DevReload` plugin adds development-mode endpoints and response headers:

```rust
AppBuilder::new()
    .build_state()
    .await
    .with(DevReload)
    // ...
```

### Dev endpoints

```
GET /__r2e_dev/status → "dev"          # Check if running in dev mode
GET /__r2e_dev/ping   → {"boot_time": 1234567890123, "status": "ok"}  # Detect restarts
```

### Dev headers

When `DevReload` is active, R2E adds two headers to every response:

- `Cache-Control: no-store` — prevents browsers from caching stale API responses
- `Connection: close` — forces the browser to close TCP connections after each response, avoiding stale keep-alive connections routed to old handler tasks after a hot-patch

### Production note

Do **not** enable `DevReload` in production. The dev endpoints are informational only but expose internal details (boot time) that shouldn't be public.

```rust
#[cfg(debug_assertions)]
builder = builder.with(DevReload);
```
