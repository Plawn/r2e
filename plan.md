# 📐 Plan d’implémentation – Surcouche Quarkus-like pour Rust (Axum)

## 🎯 Objectif

Créer une **surcouche ergonomique au-dessus d’Axum** qui offre une DX proche de Quarkus :

* Controllers déclaratifs via macros
* Injection compile-time (pas de DI runtime)
* Séparation claire app‑scoped / request‑scoped
* Support JWT / OIDC avec injection d’identité
* Zéro réflexion, zéro coût runtime inutile

Ce document est destiné à être **fourni tel quel à Claude Code** pour implémentation.

---

## 🧱 Architecture globale

### Organisation en crates

```
quarlus/
 ├─ quarlus-core/        # Runtime minimal + Axum glue
 ├─ quarlus-macros/      # Proc-macros (controller, inject, routes…)
 ├─ quarlus-security/   # JWT / Identity / OIDC
 └─ example-app/        # Exemple d’application
```

---

## 🧠 Concepts clés

### Scopes

| Scope          | Description                                         |
| -------------- | --------------------------------------------------- |
| app-scoped     | Singletons applicatifs (services, repos, clients)   |
| request-scoped | Données dérivées de la requête (identity, headers…) |

---

## 🎨 API publique cible (DX)

### Application

```rust
#[application]
struct MyApp;
```

* Marqueur logique
* Déclenche la génération de l’`AppState`
* Point d’entrée du wiring global

---

### Controller

```rust
#[controller]
impl UserResource {

    #[inject]
    user_service: UserService,

    #[identity]
    user: AuthenticatedUser,

    #[get("/users")]
    async fn list(&self) -> Json<Vec<User>> {
        self.user_service.list().await?
    }
}
```

---

### Routes supportées

```rust
#[get("/path")]
#[post("/path")]
#[put("/path")]
#[delete("/path")]
#[patch("/path")]
```

---

## 🧩 Macro `#[controller]`

### Responsabilités

* Parser un `impl` block
* Identifier :

  * champs `#[inject]`
  * champs `#[identity]`
  * méthodes annotées (`#[get]`, `#[post]`, …)
* Générer :

  * handlers Axum
  * extraction `State` + extracteurs request‑scoped
  * construction du controller

### Génération conceptuelle

Pour :

```rust
#[get("/users")]
async fn list(&self) -> Json<Vec<User>>
```

Générer :

```rust
async fn list_handler(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> impl IntoResponse {
    let controller = UserResource {
        user_service: state.user_service.clone(),
        user,
    };

    controller.list().await
}
```

---

## 🔌 `#[inject]` – App‑scoped

### Règles

* Type : `Clone + Send + Sync`
* Résolu depuis `AppState`
* Injecté lors de la construction du controller

### Implémentation

* `AppState` contient explicitement tous les services
* Pas de lookup dynamique
* Pas de map / type-id

---

## 👤 `#[identity]` – Request‑scoped

### Règles

* Type implémente `FromRequestParts`
* Injecté comme paramètre du handler Axum
* Durée de vie = requête HTTP

### Exemple

```rust
pub struct AuthenticatedUser {
    pub sub: String,
    pub email: String,
    pub roles: Vec<String>,
}
```

---

## 🔐 Sécurité – JWT / OIDC

### Flux

```
HTTP Request
 → Authorization: Bearer <jwt>
 → Extractor AuthenticatedUser
 → Vérification signature JWT
 → Validation claims
 → Injection dans controller
```

### Implémentation

* Crate `quarlus-security`
* JWKS cache (kid → clé publique)
* Rafraîchissement async
* Mapping claims → `AuthenticatedUser`

---

## 🏗️ AppState & Application Builder

### AppState

```rust
pub struct AppState {
    pub user_service: Arc<UserService>,
    pub auth_service: Arc<AuthService>,
}
```

### Builder

```rust
let app = AppBuilder::new()
    .with_config("application.yaml")
    .with_database()
    .register::<UserService>()
    .register::<AuthService>()
    .build();
```

---

## 🌐 Router final

* Routes générées automatiquement par les controllers
* Assemblées dans un `axum::Router`
* `.with_state(AppState)` appliqué globalement

---

## ⚠️ Error handling

* Handlers retournent :

  * `impl IntoResponse` ou `Result<T, E>`
* Mapping standard :

  * 401 Unauthorized
  * 403 Forbidden
  * 404 Not Found
  * 500 Internal Error

---

## 🔮 Extensions futures (non bloquantes)

* `#[roles("admin")]`
* `#[transactional]`
* `#[config]`
* OpenAPI auto
* Dev mode / hot reload

---

## ⛔ Contraintes explicites

* ❌ Pas de DI runtime
* ❌ Pas de réflexion
* ❌ Pas de macros opaques
* ✅ Génération lisible
* ✅ Erreurs de compilation exploitables

---

## 📦 Dépendances recommandées

```toml
axum
tokio
tower
tower-http
serde
sqlx
jsonwebtoken
reqwest
once_cell
syn
quote
proc-macro2
```

---

## 📦 Livrables attendus

* `quarlus-macros` fonctionnelle
* `quarlus-core` avec AppBuilder
* Exemple complet :

  * JWT valide
  * Controller avec `#[inject]` + `#[identity]`
  * Route GET fonctionnelle

---
