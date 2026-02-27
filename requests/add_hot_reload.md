# R2E — Subsecond Hot-Reload Implementation Plan

## Goal

Add subsecond Rust hot-reloading to R2E using Dioxus's **Subsecond** engine, so that `r2e dev` gives developers instant feedback on code changes — no full rebuild, no server restart, no lost connections.

Target DX:

```bash
$ r2e dev
# User edits a handler → change applied in <1s, no restart
```

---

## Context

### How Subsecond works

Subsecond (from Dioxus 0.7) patches Rust code **in-place at runtime** by recompiling only the "tip" crate as a dynamic library and hot-swapping the symbols in the running process. It requires:

1. The `dioxus-devtools` crate with the `serve` feature
2. The `dx` CLI to coordinate builds (`dx serve --hot-patch`)
3. The app entry point split into a **setup** phase (run once) and a **server** phase (hot-patched)

### Key API

```rust
dioxus_devtools::serve_subsecond_with_args(
    env,                              // persistent state (not hot-patched)
    |env| async { server(env).await } // hot-patched closure
).await;
```

### R2E's current architecture

```
r2e              — facade crate, re-exports everything
r2e-core         — AppBuilder, Controller, guards, interceptors, plugins
r2e-macros       — proc macros (#[derive(Controller)], #[routes], #[bean])
r2e-cli          — CLI: r2e new, r2e add, r2e dev, r2e generate
r2e-*            — feature crates (security, events, cache, scheduler, etc.)
```

Current user entry point:

```rust
#[tokio::main]
async fn main() {
    r2e::init_tracing();
    let config = R2eConfig::load("dev").unwrap();

    AppBuilder::new()
        .with_bean::<UserService>()
        .build_state::<AppState, _>().await
        .with_config(config)
        .with(Health)
        .with(Tracing)
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000").await.unwrap();
}
```

### Subsecond limitations to keep in mind

- Only tracks the **tip crate** — edits in workspace dependencies are not detected
- Cannot hot-patch: type signature changes, import/module structure changes
- Experimental: requires `--hot-patch` flag
- macOS and Linux well supported, Windows experimental

---

## Phase 1 — Separate `prepare()` from `serve()` in `r2e-core`

**Crate:** `r2e-core`
**Files:** `r2e-core/src/app_builder.rs`
**Effort:** Small
**Breaking changes:** None

### What to do

Expose a `PreparedApp` struct that holds a fully-built `Router` + state + address, without starting the TCP listener. The existing `.serve()` method calls `.prepare()` internally, so no breaking change.

### Implementation

Add to `r2e-core/src/app_builder.rs`:

```rust
/// A fully configured app ready to be served.
/// Separating preparation from serving enables hot-reload:
/// - `prepare()` can be called inside the hot-patched closure
/// - The setup that produces beans/config stays outside
pub struct PreparedApp<S: Clone + Send + Sync + 'static> {
    pub router: Router,
    pub state: S,
    pub addr: String,
}

impl<S: Clone + Send + Sync + 'static> PreparedApp<S> {
    /// Start listening and serving requests.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        tracing::info!("🚀 Listening on http://{}", self.addr);
        axum::serve(listener, self.router).await?;
        Ok(())
    }
}
```

Refactor the existing `AppBuilder` methods:

```rust
impl<S: Clone + Send + Sync + 'static> AppBuilder<S> {
    /// Build the app without starting the server.
    pub fn prepare(self, addr: &str) -> PreparedApp<S> {
        PreparedApp {
            router: self.build_router(), // existing internal method
            state: self.state.clone(),
            addr: addr.to_string(),
        }
    }

    /// Prepare and serve (existing behavior, unchanged).
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.prepare(addr).run().await
    }
}
```

### Acceptance criteria

- [ ] `PreparedApp` is public and documented
- [ ] `.prepare(addr)` returns a `PreparedApp`
- [ ] `.serve(addr)` still works exactly as before (calls `.prepare().run()` internally)
- [ ] `example-app` still compiles and runs with no changes
- [ ] All existing tests pass

---

## Phase 2 — Create the `r2e-devtools` crate

**Crate:** `r2e-devtools` (new)
**Files:** `r2e-devtools/Cargo.toml`, `r2e-devtools/src/lib.rs`
**Effort:** Medium

### What to do

Create a new crate that wraps Dioxus's `serve_subsecond_with_args` and provides an ergonomic API for R2E apps.

### File structure

```
r2e-devtools/
├── Cargo.toml
└── src/
    └── lib.rs
```

### `Cargo.toml`

```toml
[package]
name = "r2e-devtools"
version = "0.1.0"
edition = "2021"
description = "Subsecond hot-reload integration for R2E"

[dependencies]
dioxus-devtools = { version = "0.7", features = ["serve"] }
```

### `src/lib.rs`

```rust
//! Subsecond hot-reload integration for R2E.
//!
//! This crate should only be used in development via the `dev-reload` feature flag.
//! It wraps Dioxus's Subsecond engine to enable hot-patching of Rust code at runtime.

pub use dioxus_devtools;

/// Run an R2E app with Subsecond hot-reloading.
///
/// # Arguments
///
/// * `setup_fn` — Called **once** at startup. Use this for DB connections, config loading,
///   tracing init, and anything expensive. The returned `Env` is passed to every invocation
///   of `server_fn` and persists across hot-patches.
///
/// * `server_fn` — Called on every hot-patch. This is where you build your `AppBuilder`,
///   register controllers, and call `.serve()`. This function's code (and everything it
///   calls in the tip crate) will be hot-patched when source files change.
///
/// # Example
///
/// ```rust,no_run
/// r2e_devtools::serve_with_hotreload(
///     || async {
///         r2e::init_tracing();
///         let config = R2eConfig::load("dev").unwrap();
///         let db = setup_db().await;
///         AppEnv { config, db }
///     },
///     |env| async move {
///         AppBuilder::new()
///             .build_state::<AppState, _>().await
///             .with_config(env.config)
///             .register_controller::<UserController>()
///             .serve("0.0.0.0:3000").await.unwrap();
///     },
/// ).await;
/// ```
pub async fn serve_with_hotreload<Env, SetupFut, ServerFn, ServerFut>(
    setup_fn: impl FnOnce() -> SetupFut,
    server_fn: ServerFn,
) where
    Env: Clone + Send + Sync + 'static,
    SetupFut: std::future::Future<Output = Env>,
    ServerFn: Fn(Env) -> ServerFut + Send + 'static,
    ServerFut: std::future::Future<Output = ()> + Send,
{
    let env = setup_fn().await;
    dioxus_devtools::serve_subsecond_with_args(env, |env| async move {
        server_fn(env).await;
    })
    .await;
}
```

### Add to workspace

In the root `Cargo.toml`, add `r2e-devtools` to `[workspace.members]`:

```toml
[workspace]
members = [
    # ... existing members
    "r2e-devtools",
]
```

### Acceptance criteria

- [ ] `r2e-devtools` crate compiles
- [ ] `serve_with_hotreload` is public and documented
- [ ] Crate is added to workspace members

---

## Phase 3 — Wire the feature flag in `r2e` facade crate

**Crate:** `r2e`
**Files:** `r2e/Cargo.toml`, `r2e/src/lib.rs`
**Effort:** Small
**Breaking changes:** None

### What to do

Add an optional `dev-reload` feature that pulls in `r2e-devtools` and re-exports it. The feature must NOT be part of `full` so it's never accidentally included in production builds.

### `r2e/Cargo.toml` changes

```toml
[features]
full = ["security", "events", "scheduler", "cache", "rate-limit", "openapi", "prometheus"]
# NOTE: dev-reload is intentionally NOT in `full`
dev-reload = ["dep:r2e-devtools"]

[dependencies]
r2e-devtools = { path = "../r2e-devtools", optional = true }
```

### `r2e/src/lib.rs` changes

```rust
#[cfg(feature = "dev-reload")]
pub mod devtools {
    pub use r2e_devtools::*;
}
```

### Acceptance criteria

- [ ] `cargo build -p r2e` works without `dev-reload` (no Subsecond dependency)
- [ ] `cargo build -p r2e --features dev-reload` pulls in `r2e-devtools`
- [ ] `r2e::devtools::serve_with_hotreload` is accessible when feature is enabled
- [ ] `full` feature does NOT include `dev-reload`

---

## Phase 4 — Add `.serve_hot()` convenience method to `AppBuilder`

**Crate:** `r2e-core`
**Files:** `r2e-core/src/app_builder.rs`
**Effort:** Small
**Breaking changes:** None

### What to do

Provide a high-level method on `AppBuilder` that wraps the Subsecond setup. This is optional sugar — advanced users can use `serve_with_hotreload` directly.

### Implementation

```rust
#[cfg(feature = "dev-reload")]
impl AppBuilder<()> {
    /// Start the app with Subsecond hot-reloading.
    ///
    /// * `env` — Pre-computed environment (DB pool, config, etc.)
    /// * `build_and_serve` — Closure that builds and serves the app.
    ///   This closure (and all functions it calls in the tip crate) will be
    ///   hot-patched when source files change.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// AppBuilder::serve_hot(env, |env| async move {
    ///     AppBuilder::new()
    ///         .build_state::<AppState, _>().await
    ///         .with_config(env.config)
    ///         .register_controller::<UserController>()
    ///         .serve("0.0.0.0:3000").await.unwrap();
    /// }).await;
    /// ```
    pub async fn serve_hot<Env, F, Fut>(env: Env, build_and_serve: F)
    where
        Env: Clone + Send + Sync + 'static,
        F: Fn(Env) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        r2e_devtools::dioxus_devtools::serve_subsecond_with_args(env, |env| async move {
            build_and_serve(env).await;
        })
        .await;
    }
}
```

### Feature flag in `r2e-core/Cargo.toml`

```toml
[features]
dev-reload = ["dep:r2e-devtools"]

[dependencies]
r2e-devtools = { path = "../r2e-devtools", optional = true }
```

And propagate from `r2e/Cargo.toml`:

```toml
[features]
dev-reload = ["dep:r2e-devtools", "r2e-core/dev-reload"]
```

### Acceptance criteria

- [ ] `AppBuilder::serve_hot()` compiles when `dev-reload` feature is enabled
- [ ] `AppBuilder::serve_hot()` is not available when `dev-reload` is disabled
- [ ] Documentation includes usage example

---

## Phase 5 — Update `r2e dev` CLI command

**Crate:** `r2e-cli`
**Files:** `r2e-cli/src/commands/dev.rs` (create or modify)
**Effort:** Medium

### What to do

Make `r2e dev` orchestrate the hot-reload workflow:

1. Check that `dx` (dioxus-cli) is installed, offer to install if missing
2. Ensure a minimal `Dioxus.toml` exists (create one if not)
3. Run `dx serve --hot-patch --features dev-reload`

### Implementation

```rust
// r2e-cli/src/commands/dev.rs

use std::fs;
use std::process::Command;

pub struct DevArgs {
    pub port: Option<u16>,
    pub features: Vec<String>,
    pub release: bool,
}

pub fn run_dev(args: &DevArgs) -> anyhow::Result<()> {
    // 1. Check dx is installed
    ensure_dx_installed()?;

    // 2. Ensure Dioxus.toml exists
    ensure_dioxus_config()?;

    // 3. Build the dx command
    let mut cmd = Command::new("dx");
    cmd.args(["serve", "--hot-patch"]);

    // Merge features: always include dev-reload + any user features
    let mut features = vec!["dev-reload".to_string()];
    features.extend(args.features.clone());
    cmd.args(["--features", &features.join(",")]);

    // Forward port as env var
    if let Some(port) = args.port {
        cmd.env("R2E_PORT", port.to_string());
    }

    // 4. Run
    println!("🔥 Starting R2E dev server with hot-reload...");
    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("dx serve exited with code {}", status.code().unwrap_or(-1));
    }

    Ok(())
}

fn ensure_dx_installed() -> anyhow::Result<()> {
    match Command::new("dx").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("✅ dioxus-cli found: {}", version.trim());
            Ok(())
        }
        _ => {
            println!("⚠️  dioxus-cli not found. Installing...");
            let status = Command::new("sh")
                .args(["-c", "curl -sSL https://dioxus.dev/install.sh | bash"])
                .status()?;
            if !status.success() {
                anyhow::bail!(
                    "Failed to install dioxus-cli. Install manually: \
                     curl -sSL https://dioxus.dev/install.sh | bash"
                );
            }
            println!("✅ dioxus-cli installed successfully");
            Ok(())
        }
    }
}

fn ensure_dioxus_config() -> anyhow::Result<()> {
    let config_path = "Dioxus.toml";
    if !std::path::Path::new(config_path).exists() {
        println!("📝 Creating minimal Dioxus.toml for hot-reload...");
        fs::write(
            config_path,
            r#"[application]
name = "r2e-app"

[application.tools]
"#,
        )?;
    }
    Ok(())
}
```

### Wire into CLI's main dispatch

In the CLI's main command dispatcher, add/update the `dev` subcommand:

```rust
// r2e-cli/src/main.rs (or commands/mod.rs)
"dev" => {
    let args = DevArgs {
        port: matches.get_one::<u16>("port").copied(),
        features: matches.get_many::<String>("features")
            .map(|v| v.cloned().collect())
            .unwrap_or_default(),
        release: false,
    };
    dev::run_dev(&args)?;
}
```

### Acceptance criteria

- [ ] `r2e dev` checks for `dx` and installs it if missing
- [ ] `r2e dev` creates `Dioxus.toml` if it doesn't exist
- [ ] `r2e dev` passes `--hot-patch` and `--features dev-reload` to `dx`
- [ ] `r2e dev --port 8080` forwards the port via env var
- [ ] `r2e dev --features my-feature` merges with `dev-reload`

---

## Phase 6 — Update `example-app` with hot-reload support

**Crate:** `example-app`
**Files:** `example-app/src/main.rs`, `example-app/Cargo.toml`
**Effort:** Small

### What to do

Show both modes (normal and hot-reload) in the example app, serving as documentation and a test bed.

### `example-app/Cargo.toml`

```toml
[features]
dev-reload = ["r2e/dev-reload"]
```

### `example-app/src/main.rs`

```rust
use r2e::prelude::*;

// --- Setup: runs once, not hot-patched ---

#[derive(Clone)]
struct AppEnv {
    config: R2eConfig,
}

async fn setup() -> AppEnv {
    r2e::init_tracing();
    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());
    AppEnv { config }
}

// --- Server: hot-patched on every code change ---

async fn server(env: AppEnv) {
    AppBuilder::new()
        .with_bean::<UserService>()
        .build_state::<AppState, _>().await
        .with_config(env.config)
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .register_controller::<UserController>()
        .serve("0.0.0.0:3001").await.unwrap();
}

// --- Entry points ---

#[cfg(not(feature = "dev-reload"))]
#[tokio::main]
async fn main() {
    let env = setup().await;
    server(env).await;
}

#[cfg(feature = "dev-reload")]
#[tokio::main]
async fn main() {
    r2e::devtools::serve_with_hotreload(setup, server).await;
}
```

### Acceptance criteria

- [ ] `cargo run -p example-app` works as before (no hot-reload)
- [ ] `r2e dev` (from example-app directory) starts with hot-reload
- [ ] Modifying a handler in `example-app/src/` triggers a subsecond hot-patch
- [ ] The example serves as clear documentation for users

---

## Phase 7 (Optional) — Enrich `DevReload` plugin with browser live-reload

**Crate:** `r2e-core`
**Files:** `r2e-core/src/plugins/dev_reload.rs`
**Effort:** Medium
**Priority:** Nice-to-have, can be done later

### What to do

If R2E is used to serve HTML (not just a JSON API), users benefit from automatic browser refresh on hot-patch. Enhance the existing `DevReload` plugin with:

1. An SSE endpoint at `/__r2e_dev/reload` that emits an event after each hot-patch
2. A tiny JS snippet injected into HTML responses that listens to the SSE stream
3. On receiving a reload event, the browser refreshes automatically

### Implementation sketch

```rust
// r2e-core/src/plugins/dev_reload.rs

use axum::{response::sse::{Event, Sse}, Router, routing::get};
use tokio::sync::broadcast;
use std::convert::Infallible;
use futures_util::stream::Stream;

/// Global broadcast channel for reload events
static RELOAD_TX: once_cell::sync::Lazy<broadcast::Sender<()>> =
    once_cell::sync::Lazy::new(|| broadcast::channel(16).0);

/// Call this after a hot-patch completes to notify all connected browsers.
pub fn notify_reload() {
    let _ = RELOAD_TX.send(());
}

/// SSE endpoint that browsers connect to.
async fn reload_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = RELOAD_TX.subscribe();
    let stream = async_stream::stream! {
        while let Ok(()) = rx.recv().await {
            yield Ok(Event::default().event("reload").data(""));
        }
    };
    Sse::new(stream)
}

/// JS snippet to inject in HTML responses.
pub const LIVE_RELOAD_SCRIPT: &str = r#"
<script>
(function() {
    const es = new EventSource('/__r2e_dev/reload');
    es.addEventListener('reload', () => window.location.reload());
    es.onerror = () => setTimeout(() => window.location.reload(), 1000);
})();
</script>
"#;

impl<S: Clone + Send + Sync + 'static> Plugin<S> for DevReload {
    fn apply(self, router: Router<S>) -> Router<S> {
        router
            .route("/__r2e_dev/status", get(dev_status))
            .route("/__r2e_dev/reload", get(reload_sse))
            // Optionally: layer that injects LIVE_RELOAD_SCRIPT before </body>
    }
}
```

### Acceptance criteria

- [ ] `/__r2e_dev/reload` SSE endpoint exists when `DevReload` plugin is active
- [ ] Browsers connected to the SSE stream refresh on hot-patch
- [ ] No impact when `DevReload` plugin is not registered

---

## Summary

| Phase | Crate(s) | Effort | Description |
|-------|----------|--------|-------------|
| 1 | `r2e-core` | Small | Separate `prepare()` / `serve()` in AppBuilder |
| 2 | `r2e-devtools` (new) | Medium | Subsecond wrapper crate |
| 3 | `r2e` | Small | Feature flag `dev-reload` in facade |
| 4 | `r2e-core` | Small | `.serve_hot()` convenience method |
| 5 | `r2e-cli` | Medium | `r2e dev` → `dx serve --hot-patch` |
| 6 | `example-app` | Small | Example with both modes |
| 7 | `r2e-core` | Optional | Browser live-reload via SSE |

### Recommended execution order

Phases 1–3 can be done in a single PR (foundational plumbing).
Phase 4 in a second PR (ergonomic API).
Phase 5 in a third PR (CLI integration).
Phase 6 alongside phase 5 (example update).
Phase 7 can be a separate follow-up PR.

### `.gitignore` addition

Add `Dioxus.toml` to `.gitignore` if it's auto-generated by `r2e dev`, or commit a default one at the repo root.

### Documentation to update

- [ ] Root `README.md` — mention hot-reload in Features section
- [ ] Add a `docs/hot-reload.md` guide explaining the setup/server split pattern
- [ ] `r2e-cli` help text for `r2e dev`
- [ ] Inline doc comments on all public APIs