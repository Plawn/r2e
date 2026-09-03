---
topic: quick-start
features: default
tokens: ~2100
requires: runtime-facade
---

## Quick Start

### TL;DR

- Depend on `r2e` alone — no `tokio` and no `axum` entry in `Cargo.toml`.
- Enable what you need with `r2e` features; `full` turns on everything except `dev-reload`. Validation needs no feature.
- Declare the app with the `App` trait in the single canonical `src/app.rs`; `src/lib.rs` is `include!("app.rs")` and `src/main.rs` is `r2e::app_main!(MyApp);`.
- `App::setup` builds process-lifetime resources (the `Env`, survives hot-patches); `App::build` assembles the builder and is re-run on every hot-patch.
- Both phases are fallible: return `BootError` with `?`, never call `std::process::exit` in `setup`/`build`.
- `load_config` is the sole config registration point that reads disk (`provide_config` is the in-memory counterpart).
- End `build` with `.try_build_state().await?` then `.register_controller::<C>()`, and return that value — do not serve there.
- `app_main!` installs the subscriber from the app's own `tracing:` section after `setup`; use `app_main!(MyApp, tracing = false)` when a plugin must own the subscriber instead.
- Never write a state struct and never put `state = ...` on `#[controller]`: the state is inferred from `.provide()` / `.register()`.
- Test the real app with `#[r2e::test(app = my_app::MyApp)]` taking a `TestApp`.

### Cargo.toml

```toml
[dependencies]
r2e = { version = "0.3", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

No `tokio` entry: `r2e::main` / `r2e::app_main!` build the runtime through
`r2e::rt`, and everything an app needs from the runtime (`spawn`, `sleep`,
`select!`, `sync::*`, `CancelToken`, …) is re-exported there.
See llm/runtime-facade.md.

Feature flags: `security`, `events`, `utils` (on by default), `data`, `data-sqlx`, `data-diesel`, `sqlite`, `postgres`, `mysql`, `scheduler` (implies `executor`), `executor`, `cache`, `rate-limit`, `openapi`, `oidc`, `prometheus`, `metrics-facade` (HTTP metrics through the `metrics` crate instead of the `prometheus` registry; not in `full`), `openfga`, `observability`, `grpc`, `grpc-reflection`, `grpc-web` (grpc-web on the multiplexed port via tonic-web), `multipart`, `ws`, `static`, `tenant`, `tenant-sqlx` (per-tenant SQLx pools; not in `full`), `tenant-diesel` (per-tenant Diesel pools; not in `full`), `mcp` (MCP server), `quic`, plus distributed event backends (`events-iggy`, `events-kafka`, `events-pulsar`, `events-rabbitmq`). `full` enables all (except `dev-reload`). Validation (via `garde`) is always available — no feature flag needed.

### Minimal Application — the `App` trait

**Always declare an R2E app via the `App` trait** (in the prelude). Keep its
single canonical source in `src/app.rs`: `lib.rs` includes it for integration
tests, while `r2e::app_main!(MyApp)` includes that same file directly in the
binary tip crate and generates `main`. There is one declaration of `MyApp`, used
by production, tests, and real hot-reload; users write no target `cfg` or
crate-name-dependent import.

- `App::setup() -> Result<Self::Env, BootError>` — long-lived resources built
  **once** (pools, buses). Under the `dev-reload` feature the `Env` survives
  hot-patches. Put its type and setup helpers in `src/env.rs`, a cold boundary
  that `r2e dev` fully restarts when changed. Nothing to persist? Use an empty
  `AppEnv` as shown below.
- `App::build(b, env) -> Result<impl BootableApp, BootError>` — assembles the app
  on the builder; **re-run on every hot-patch**. `load_config` is the sole config
  registration point that reads disk (`provide_config` is its in-memory
  counterpart for settings already in hand).
- **Both phases are fallible.** `BootError` is
  `Box<dyn std::error::Error + Send + Sync>`, so `?` accepts any `std` error.
  Never call `std::process::exit` in `setup`/`build`: that code is linked by the
  test harness, and an `exit` there kills the whole test binary with no
  attributable failure and no `Drop` for anything already built. Return the error
  — `app_main!` prints one `error:` line plus the `source()` chain and exits `1`,
  and `TestApp::boot` turns it into a failing test naming the cause.
- Under the hot-patch loop, bean instances also survive patches: only beans whose
  constructor code or declared config values changed (plus their transitive
  dependents) are reconstructed; everything else — including `.provide()`-ed
  values — carries its in-memory state over. `r2e::invalidate_state_cache()`
  forces a cold rebuild.

```rust,ignore
// src/app.rs — the one canonical application source
use r2e::prelude::*;

pub mod controllers;
pub mod env;

use controllers::hello::HelloController;
use env::{setup_env, AppEnv};

pub struct MyApp;

impl App for MyApp {
    type Env = AppEnv;

    async fn setup() -> Result<AppEnv, BootError> {
        setup_env().await
    }

    async fn build(b: AppBuilder, _env: Self::Env) -> Result<impl BootableApp, BootError> {
        Ok(b
            .load_config::<()>()                 // application.yaml + env (sole config entry)
            .plugin(Health)                      // /health → 200 "OK"
            .plugin(HttpTrace::new())            // one span + one log line per request
            .try_build_state()                   // resolve bean graph (state is inferred)
            .await?                              // a bean that fails to build aborts boot
            .register_controller::<HelloController>())
    }
}
```

```rust
// src/env.rs — cold, process-lifetime resources
#[derive(Clone, Default)]
pub struct AppEnv;

pub async fn setup_env() -> Result<AppEnv, r2e::BootError> {
    Ok(AppEnv)
}
```

```rust,ignore
// src/lib.rs — integration tests
include!("app.rs");
```

```rust,ignore
// src/main.rs — prod serve AND dev hot-reload
r2e::app_main!(MyApp);                          // include app.rs + generate main + launch
// r2e::app_main!(MyApp, tracing = false);      // ...and don't install a subscriber
```

`app_main!` installs the global `tracing` subscriber **after** `App::setup`
returns (and, under `dev-reload`, once — outside the patch loop), so an app that
builds its own subscriber in `setup` wins; the install is idempotent. Anything
`setup` logs before that point is dropped. What it installs is the app's own
`tracing:` section (`init_tracing_from_config()`), so `format: json` in
`application.yaml` applies from the first log line, with no plugin and no opt-in;
an unreadable section falls back to the built-in defaults and warns.

The subscriber is only *where logs go*: per-request spans and the
`request completed` line come from `.plugin(HttpTrace::new())` (see
llm/observability.md), which is what an app installs in `build`.

Because that happens before `App::build`, a `Tracing::from_config(&cfg)` /
`ConfiguredTracing` plugin declared in `build` **loses the race** — silently when
it reads the same section (same subscriber either way), with a warning naming the
ignored format/filter when it would have logged differently. To let the plugin
own the subscriber, opt the entry point out: `app_main!(MyApp, tracing = false)`
— same spelling as `#[r2e::main(tracing = false)]` — installs nothing at all.
`launch!(MyApp, tracing = false)` is the same knob for a custom `main`.

```rust
// src/controllers/hello.rs
use r2e::prelude::*;

#[controller(path = "/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> Json<&'static str> {
        Json("Hello, world!")
    }
}
# fn main() {}
```

```rust
// tests/app.rs — boots the REAL app
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};

#[r2e::test(app = my_app::MyApp)]                // app = a TYPE implementing App
async fn hello_works(app: TestApp) {
    app.get("/hello").send().await.assert_ok();
}
```

**There is no hand-written state struct.** The application state is inferred
from what you `.provide()` / `.register()` — a compile-time HList, installed on
the router as `BeanState<L>` (that list behind one `Arc`, so each per-request
state clone the HTTP backend performs is one refcount bump instead of one
clone per bean). You never
name that type: there is no state derive to write, and `state = ...` on
`#[controller]` does not exist.
