# Your First App

This guide walks you through building a simple REST API with R2E from scratch.

## 1. Create the project

```bash
cargo new my-api
cd my-api
```

Add dependencies to `Cargo.toml`:

```toml
[dependencies]
r2e = { version = "0.1" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

## 2. Understand application state

There is **no state struct to write**. R2E infers your application's state from
the bean graph: every value you `.provide(...)` and every type you
`.register::<T>()` becomes a bean, and `build_state()` materializes them into an
inferred state that controllers draw from *by type*. Config (`R2eConfig`) is
registered as a bean automatically when you call `load_config` — so a minimal app
has nothing to define here and you can go straight to writing a controller.

## 3. Create a controller

Create `src/controllers/mod.rs`:
```rust
pub mod hello;
```

Create `src/controllers/hello.rs`:

```rust
use r2e::prelude::*;

#[controller(path = "/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        "Hello, R2E!"
    }

    #[get("/{name}")]
    async fn greet(&self, Path(name): Path<String>) -> String {
        format!("Hello, {}!", name)
    }
}
```

## 4. Define the application once

Create `src/env.rs` for resources that must survive hot-patches:

```rust
#[derive(Clone, Default)]
pub struct AppEnv;

pub async fn setup_env() -> AppEnv {
    AppEnv
}
```

Create the canonical `src/app.rs`:

```rust
use r2e::prelude::*;

pub mod controllers;
pub mod env;

use controllers::hello::HelloController;
use env::{setup_env, AppEnv};

pub struct MyApp;

impl App for MyApp {
    type Env = AppEnv;

    async fn setup() -> AppEnv {
        setup_env().await
    }

    async fn build(b: AppBuilder, _env: AppEnv) -> impl BootableApp {
        b.load_config::<()>()
            .build_state().await
            .with(Health)
            .with(Tracing)
            .register_controller::<HelloController>()
    }
}
```

Expose the same app type to integration tests from `src/lib.rs`:

```rust
include!("app.rs");
```

Then replace `src/main.rs` with the standard entrypoint:

```rust
r2e::app_main!(MyApp);
```

`app_main!` includes the same `app.rs` in the binary, generates Tokio `main`,
and handles normal serving or hot reload. `build_state()` takes no type
arguments; controllers are registered after it.

> **Large apps:** if your bean graph grows past ~127 registrations, add
> `#![recursion_limit = "512"]` at the top of both `main.rs` and `lib.rs`.
> `r2e doctor` warns as you approach the threshold.

## 5. Run it

```bash
cargo run
```

Test with curl:

```bash
curl http://localhost:3000/hello
# Hello, R2E!

curl http://localhost:3000/hello/World
# Hello, World!

curl http://localhost:3000/health
# OK
```

## Adding a service

Let's add a simple in-memory user service. Create `src/services.rs`:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use r2e::prelude::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Clone)]
pub struct UserService {
    users: Arc<RwLock<Vec<User>>>,
}

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self {
            users: Arc::new(RwLock::new(vec![
                User { id: 1, name: "Alice".into() },
                User { id: 2, name: "Bob".into() },
            ])),
        }
    }

    pub async fn list(&self) -> Vec<User> {
        self.users.read().await.clone()
    }

    pub async fn create(&self, name: String) -> User {
        let mut users = self.users.write().await;
        let id = users.len() as u64 + 1;
        let user = User { id, name };
        users.push(user.clone());
        user
    }
}
```

Create `src/controllers/user.rs`:

```rust
use crate::services::{User, UserService};
use r2e::prelude::*;

#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,   // resolved from the bean graph by type
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[post("/")]
    async fn create(&self, Json(body): Json<serde_json::Value>) -> Json<User> {
        let name = body["name"].as_str().unwrap_or("Anonymous").to_string();
        Json(self.user_service.create(name).await)
    }
}
```

No state struct to edit — registering `UserService` in `App::build` (next step)
is all it takes. The controller's `#[inject] user_service: UserService`
field is resolved from the graph by type.

Update `src/controllers/mod.rs`:
```rust
pub mod hello;
pub mod user;
```

Update `src/app.rs`:

```rust
use r2e::prelude::*;

pub mod controllers;
pub mod env;
pub mod services;

use controllers::hello::HelloController;
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
            .register::<services::UserService>()
            .build_state().await
            .with(Health)
            .with(Tracing)
            .register_controllers::<(HelloController, UserController)>()
    }
}
```

Multiple controllers can be registered in one call with the tuple form
`.register_controllers::<(A, B, ...)>()` (arity 1..=16), or one at a time with
`.register_controller::<T>()`.

Test:

```bash
curl http://localhost:3000/users
# [{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]

curl -X POST http://localhost:3000/users -H "Content-Type: application/json" -d '{"name":"Charlie"}'
# {"id":3,"name":"Charlie"}
```

## Next steps

- [CLI Scaffolding](./cli-scaffolding.md) — use `r2e new` for faster project setup
- [Controllers](../core-concepts/controllers.md) — learn all controller features
- [Dependency Injection](../core-concepts/dependency-injection.md) — understand injection scopes
