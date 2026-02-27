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

3. Structure your app with setup/server split:

```rust
#[derive(Clone)]
struct AppEnv {
    pool: PgPool,
    config: R2eConfig,
}

async fn setup() -> AppEnv {
    // runs once, persists across hot-patches
    let pool = PgPool::connect("...").await.unwrap();
    AppEnv { pool, config: R2eConfig::load("dev").unwrap() }
}

#[r2e::main]
async fn main(env: AppEnv) {
    // this body is hot-patched on every code change
    AppBuilder::new()
        .provide(env.pool)
        .build_state::<MyState, _, _>().await
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

## Legacy polling (DevReload plugin)

The `DevReload` plugin exposes `/__r2e_dev/ping` for restart detection. This is still available for tools that poll for server restarts.

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

## Production note

Do **not** enable `DevReload` in production. The dev endpoints are informational only but expose internal details (boot time) that shouldn't be public.

```rust
#[cfg(debug_assertions)]
builder = builder.with(DevReload);
```
