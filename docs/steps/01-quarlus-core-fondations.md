# Etape 1 — quarlus-core : fondations

## Objectif

Implementer le coeur du framework : `AppState`, `AppBuilder`, error handling, et le trait `Controller` qui servira de base au code genere par les macros.

## Fichiers a creer

```
quarlus-core/src/
  lib.rs          # Re-exports publics
  state.rs        # AppState generique
  builder.rs      # AppBuilder
  error.rs        # Types d'erreurs + IntoResponse
  controller.rs   # Trait Controller
```

## 1. AppState (`state.rs`)

L'AppState est un conteneur generique wrappant les services applicatifs.

```rust
use std::sync::Arc;

/// Conteneur d'etat applicatif.
/// T est une struct definie par l'utilisateur contenant ses services.
#[derive(Clone)]
pub struct AppState<T: Clone + Send + Sync + 'static> {
    inner: Arc<T>,
}

impl<T: Clone + Send + Sync + 'static> AppState<T> {
    pub fn new(inner: T) -> Self {
        Self { inner: Arc::new(inner) }
    }

    pub fn get(&self) -> &T {
        &self.inner
    }
}
```

### Pourquoi generique ?

Chaque application definit ses propres services. L'`AppState<T>` permet de garder le typage fort sans recourir a `TypeId` ou `Any`.

## 2. AppBuilder (`builder.rs`)

Builder fluide pour construire l'application Axum.

```rust
pub struct AppBuilder<T: Clone + Send + Sync + 'static> {
    state: Option<T>,
    routes: Vec<axum::Router<AppState<T>>>,
}

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    pub fn new() -> Self { ... }

    /// Definit le state applicatif
    pub fn with_state(mut self, state: T) -> Self { ... }

    /// Ajoute un router (genere par un controller)
    pub fn register_routes(mut self, router: axum::Router<AppState<T>>) -> Self { ... }

    /// Construit le Router final
    pub fn build(self) -> axum::Router { ... }

    /// Construit et demarre le serveur
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> { ... }
}
```

### Comportement de `build()`

1. Merge tous les routers enregistres
2. Applique `.with_state(AppState::new(state))`
3. Retourne un `axum::Router` pret a servir

## 3. Error handling (`error.rs`)

```rust
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;

/// Erreur applicative standard
pub enum AppError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound(msg)     => (StatusCode::NOT_FOUND, msg),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            AppError::Forbidden(msg)    => (StatusCode::FORBIDDEN, msg),
            AppError::BadRequest(msg)   => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg)     => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}
```

Fournir aussi des conversions `From<sqlx::Error>`, `From<std::io::Error>`, etc. pour permettre l'usage de `?` dans les handlers.

## 4. Trait Controller (`controller.rs`)

```rust
/// Trait que les controllers generes implementent.
/// Fournit la methode pour enregistrer les routes.
pub trait Controller<T: Clone + Send + Sync + 'static> {
    /// Retourne le Router contenant toutes les routes de ce controller.
    fn routes() -> axum::Router<AppState<T>>;
}
```

Ce trait sera implemente automatiquement par la macro `#[controller]`.

## 5. Re-exports (`lib.rs`)

```rust
pub mod state;
pub mod builder;
pub mod error;
pub mod controller;

pub use state::AppState;
pub use builder::AppBuilder;
pub use error::AppError;
pub use controller::Controller;
```

## Critere de validation

```bash
cargo check -p quarlus-core
```

Compile sans erreur. Les types publics sont accessibles depuis `quarlus_core::*`.

## Dependances entre etapes

- Requiert : etape 0 (workspace setup)
- Bloque : etapes 2, 3, 4
