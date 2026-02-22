# Etape 6 — Application exemple complete

## Objectif

Creer une application de demonstration fonctionnelle qui utilise tous les composants du framework : controllers, injection, identite JWT, routes, et serveur HTTP.

## Structure

```
example-app/
  Cargo.toml
  src/
    main.rs             # Point d'entree
    services.rs         # Services applicatifs (app-scoped)
    controllers/
      mod.rs
      user_controller.rs
      health_controller.rs
    models.rs           # Structs de donnees
    state.rs            # Definition de l'AppState applicatif
```

## 1. Modeles (`models.rs`)

```rust
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}
```

## 2. Services (`services.rs`)

```rust
use crate::models::User;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct UserService {
    // Store in-memory pour la demo
    users: Arc<RwLock<Vec<User>>>,
}

impl UserService {
    pub fn new() -> Self {
        let users = vec![
            User { id: 1, name: "Alice".into(), email: "alice@example.com".into() },
            User { id: 2, name: "Bob".into(), email: "bob@example.com".into() },
        ];
        Self { users: Arc::new(RwLock::new(users)) }
    }

    pub async fn list(&self) -> Vec<User> {
        self.users.read().await.clone()
    }

    pub async fn get_by_id(&self, id: u64) -> Option<User> {
        self.users.read().await.iter().find(|u| u.id == id).cloned()
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let mut users = self.users.write().await;
        let id = users.len() as u64 + 1;
        let user = User { id, name, email };
        users.push(user.clone());
        user
    }
}
```

## 3. State applicatif (`state.rs`)

```rust
use crate::services::UserService;

#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,
}
```

## 4. Controllers

### Health controller (`controllers/health_controller.rs`)

```rust
use r2e_macros::{controller, get};

pub struct HealthController;

#[controller(state = crate::state::Services)]
impl HealthController {
    #[get("/health")]
    async fn health(&self) -> String {
        "OK".to_string()
    }
}
```

### User controller (`controllers/user_controller.rs`)

```rust
use r2e::prelude::*; // Controller, get, post, Json, Path
use r2e::r2e_security::AuthenticatedUser;
use crate::services::UserService;
use crate::models::User;

pub struct UserController;

#[controller(state = crate::state::Services)]
impl UserController {
    #[inject]
    user_service: UserService,

    #[identity]
    user: AuthenticatedUser,

    #[get("/users")]
    async fn list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }

    #[get("/users/:id")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, r2e_core::HttpError> {
        match self.user_service.get_by_id(id).await {
            Some(user) => Ok(Json(user)),
            None => Err(r2e_core::HttpError::NotFound("User not found".into())),
        }
    }

    #[post("/users")]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
    }

    #[get("/me")]
    async fn me(&self) -> Json<AuthenticatedUser> {
        Json(self.user.clone())
    }
}

#[derive(serde::Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}
```

## 5. Point d'entree (`main.rs`)

```rust
use r2e_core::AppBuilder;
use r2e_security::SecurityConfig;

mod models;
mod services;
mod state;
mod controllers;

use controllers::health_controller::HealthController;
use controllers::user_controller::UserController;
use services::UserService;
use state::Services;

#[tokio::main]
async fn main() {
    let services = Services {
        user_service: UserService::new(),
    };

    AppBuilder::new()
        .with_state(services)
        .register_controller::<HealthController>()
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

## 6. Tests manuels

```bash
# Health check
curl http://localhost:3000/health
# → "OK"

# Liste des utilisateurs (avec JWT)
curl -H "Authorization: Bearer <jwt>" http://localhost:3000/users
# → [{"id":1,"name":"Alice",...}, {"id":2,"name":"Bob",...}]

# Utilisateur par ID
curl -H "Authorization: Bearer <jwt>" http://localhost:3000/users/1
# → {"id":1,"name":"Alice","email":"alice@example.com"}

# Creation
curl -X POST -H "Content-Type: application/json" \
     -H "Authorization: Bearer <jwt>" \
     -d '{"name":"Charlie","email":"charlie@example.com"}' \
     http://localhost:3000/users
# → {"id":3,"name":"Charlie","email":"charlie@example.com"}

# Identite
curl -H "Authorization: Bearer <jwt>" http://localhost:3000/me
# → {"sub":"user123","email":"test@example.com","roles":["user"]}
```

## Critere de validation

L'application compile et repond correctement aux requetes HTTP ci-dessus.

## Dependances entre etapes

- Requiert : toutes les etapes precedentes (0-5)
