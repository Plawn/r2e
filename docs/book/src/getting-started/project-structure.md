# Project Structure

A typical R2E project follows this layout:

```
my-app/
├── Cargo.toml
├── application.yaml              # Configuration
├── migrations/                   # SQL migrations (if using data features)
│   └── 20250101000001_init.sql
├── src/
│   ├── app.rs                    # Canonical App implementation and DI graph
│   ├── env.rs                    # Cold resources retained across hot-patches
│   ├── lib.rs                    # Includes app.rs for integration tests
│   ├── main.rs                   # r2e::app_main!(MyApp);
│   ├── error.rs                  # Custom error type (optional)
│   ├── models/
│   │   ├── mod.rs
│   │   └── user.rs               # Domain models and request/response types
│   ├── services/
│   │   ├── mod.rs
│   │   └── user_service.rs       # Business logic
│   ├── controllers/
│   │   ├── mod.rs
│   │   └── user_controller.rs    # HTTP handlers
│   └── middleware/                # Custom interceptors (optional)
│       ├── mod.rs
│       └── audit_log.rs
└── tests/
    └── user_test.rs              # Integration tests
```

## Key files

### `main.rs` — Application entry point

The conventional entry point is deliberately one line:

```rust
r2e::app_main!(MyApp);
```

The macro includes `src/app.rs` in the binary tip crate, generates the Tokio
`main`, and invokes the normal or hot-reload launch path. A custom
`#[r2e::main]` plus `r2e::launch!` remains available when needed.

### `app.rs` — Canonical application assembly

Production, dev, and tests share this one `App` implementation:

```rust
use r2e::prelude::*;

pub mod controllers;
pub mod env;

use controllers::user::UserController;
use env::{setup_env, AppEnv};

pub struct MyApp;

impl App for MyApp {
    type Env = AppEnv;

    async fn setup() -> AppEnv {
        setup_env().await
    }

    async fn build(b: AppBuilder, _env: AppEnv) -> impl BootableApp {
        b.load_config::<()>()
            .register::<UserService>()
            .build_state().await
            .with(Health)
            .with(Tracing)
            .register_controller::<UserController>()
    }
}
```

`build_state()` takes no type arguments — the state type is inferred from the
registered beans. If your bean graph grows past ~127 registrations, add
`#![recursion_limit = "512"]` to the crate roots in `main.rs` and `lib.rs`
(`r2e doctor` warns as you approach the threshold).

### `env.rs` and `lib.rs`

`env.rs` owns process-lifetime pools, buses, and clients created by
`App::setup`; `r2e dev` fully restarts when its layout changes. `lib.rs` is the
thin test adapter:

```rust
include!("app.rs");
```

### Application state — inferred, no struct to write

R2E has **no hand-written state struct**. Application state is the *inferred* set
of beans: each value you `.provide(...)` and each type you `.register::<T>()`
forms the bean graph, and `build_state()` materializes it into a state that
controllers and plugins read from *by type*. Config (`R2eConfig`) and typed
`#[config(section)]` children are registered as beans automatically by
`load_config`. There is no `state.rs` file in an R2E project.

### Controllers

Controllers are structs with `#[controller]` and an impl block with `#[routes]`. Each method becomes an Axum handler:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,   // resolved from the bean graph by type
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> { ... }
}
```

### Services

Services contain business logic and are injected into controllers via `#[inject]`. Use `#[bean]` on the impl block to register with the DI system:

```rust
#[derive(Clone)]
pub struct UserService { ... }

#[bean]
impl UserService {
    pub fn new(pool: SqlitePool) -> Self { ... }
}
```

### Models

Model files define data types: entities (database-mapped), request/response DTOs, and events:

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct User { pub id: i64, pub name: String }

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[garde(length(min = 1))]
    pub name: String,
}
```

### Configuration files

YAML file with env var overlay:

```yaml
# application.yaml
app:
  name: "my-app"
database:
  url: "sqlite:data.db"
```

## Convention over configuration

R2E follows a few conventions:
- Controllers live in `src/controllers/` and are registered explicitly via `register_controller::<T>()`
- Services live in `src/services/` and are registered as beans via `register::<T>()`
- Configuration is loaded from `application.yaml` (with env var overlay)
- There is no state struct — application state is the inferred bean graph, materialized by `build_state()`
