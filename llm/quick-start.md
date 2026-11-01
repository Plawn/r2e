---
topic: quick-start
features: default
tokens: ~2000
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
- `build_state()` installs the subscriber from the app's own `tracing:` section (no plugin, no macro argument); `tracing.enabled: false` only when the app installs a subscriber of its own (`Observability` does not need it: it contributes its layers to R2E's).
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
```

`build_state()` installs the global `tracing` subscriber, first thing, from
the app's own `tracing:` section — so `format: json` in `application.yaml`
applies from the first log line, with no plugin and no opt-in; an unreadable
section falls back to the built-in defaults and logs one ERROR. What `setup`
logs before that is buffered by `app_main!` (what `RUST_LOG` enables, else INFO and above; 1024 events) and
replayed through the real subscriber once it is installed.

The subscriber is only *where logs go*: per-request spans and the
`request completed` line come from `.plugin(HttpTrace::new())` (see
llm/observability.md), which is what an app installs in `build`. Layers
(`Suppress`, a layer writing `LogFields`, an extra `fmt` layer) are contributed
with `.tracing_layer(..)` before `build_state()`.

To install a subscriber of your own, set `tracing.enabled: false`: R2E then
installs nothing at all (plugins that only contribute a layer, like
`Observability`, need no opt-out). There is no `tracing = …` macro argument.

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
