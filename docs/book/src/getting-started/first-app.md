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

## 2. Define your state

Create `src/state.rs`:

```rust
use r2e::prelude::*;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub config: R2eConfig,
}
```

`BeanState` derives the `FromRef` implementations needed for Axum's state extraction.

## 3. Create a controller

Create `src/controllers/mod.rs`:
```rust
pub mod hello;
```

Create `src/controllers/hello.rs`:

```rust
use crate::state::AppState;
use r2e::prelude::*;

#[derive(Controller)]
#[controller(path = "/hello", state = AppState)]
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

## 4. Wire it up in main

Replace `src/main.rs`:

```rust
use r2e::prelude::*;
use r2e::plugins::{Health, Tracing};

mod controllers;
mod state;

use controllers::hello::HelloController;
use state::AppState;

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    AppBuilder::new()
        .build_state::<AppState, _>()
        .await
        .with(Health)
        .with(Tracing)
        .register_controller::<HelloController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

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
use crate::state::AppState;
use crate::services::{User, UserService};
use r2e::prelude::*;

#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
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

Update `src/state.rs` to include the service:

```rust
use r2e::prelude::*;
use crate::services::UserService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub user_service: UserService,
    pub config: R2eConfig,
}
```

Update `src/controllers/mod.rs`:
```rust
pub mod hello;
pub mod user;
```

Update `src/main.rs`:

```rust
use r2e::prelude::*;
use r2e::plugins::{Health, Tracing};

mod controllers;
mod services;
mod state;

use controllers::hello::HelloController;
use controllers::user::UserController;
use state::AppState;

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    AppBuilder::new()
        .with_bean::<services::UserService>()
        .build_state::<AppState, _>()
        .await
        .with(Health)
        .with(Tracing)
        .register_controller::<HelloController>()
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

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
