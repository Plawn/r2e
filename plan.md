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
r2e/
 ├─ r2e-macros/       # Proc-macros (controller, inject, routes…)
 ├─ r2e-core/         # Runtime minimal + Axum glue + AppBuilder + config + guards + intercepteurs
 ├─ r2e-security/     # JWT / Identity / OIDC / JWKS
 ├─ r2e-events/       # EventBus pub/sub typé
 ├─ r2e-scheduler/    # Tâches planifiées (interval, cron, delay)
 ├─ r2e-data/         # Entity, QueryBuilder, Repository, Pageable/Page
 ├─ r2e-cache/        # TtlCache, CacheStore trait, InMemoryStore
 ├─ r2e-rate-limit/   # RateLimiter token-bucket, RateLimitRegistry
 ├─ r2e-openapi/      # Génération OpenAPI 3.0.3 + Swagger UI
 ├─ r2e-utils/        # Intercepteurs built-in (Logged, Timed, Cache, CacheInvalidate)
 ├─ r2e-test/         # TestApp, TestJwt pour tests d'intégration
 ├─ r2e-cli/          # CLI : r2e new/add/dev/generate
 └─ example-app/          # Application démo complète
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

* Crate `r2e-security`
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

*Toutes implémentées :*

* ✅ `#[roles("admin")]` — guard de rôles (r2e-security + r2e-macros)
* ✅ `#[transactional]` — wrapping SQL transaction automatique (r2e-macros)
* ✅ `#[config("key")]` — injection de configuration (r2e-core + r2e-macros)
* ✅ OpenAPI auto — génération spec 3.0.3 + Swagger UI (r2e-openapi)
* ✅ Dev mode / hot reload — endpoints `/__r2e_dev/*` (r2e-core)

*Ajouts supplémentaires réalisés :*

* ✅ `#[rate_limited]` — rate limiting par token bucket (r2e-rate-limit)
* ✅ `#[intercept(...)]` — intercepteurs (Logged, Timed, Cache, CacheInvalidate + custom)
* ✅ `#[guard(...)]` — guards custom (r2e-core)
* ✅ `#[consumer(bus = "...")]` — consommateurs d'événements (r2e-events)
* ✅ `#[scheduled(every/cron)]` — tâches planifiées (r2e-scheduler)
* ✅ `#[middleware(...)]` — middleware Tower par route
* ✅ Data/Repository — Entity, QueryBuilder, Pageable, Page (r2e-data)
* ✅ Cache pluggable — CacheStore trait + InMemoryStore (r2e-cache)
* ✅ Test helpers — TestApp, TestJwt (r2e-test)
* ✅ CLI — r2e new/add/dev/generate (r2e-cli)
* ✅ Lifecycle hooks — on_start / on_stop (r2e-core)
* ✅ Validation — Validated<T> extractor (r2e-core, feature-gated)

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

*Tous livrés :*

* ✅ `r2e-macros` — `#[derive(Controller)]` + `#[routes]` avec tous les attributs
* ✅ `r2e-core` — AppBuilder, Controller, Guard, Interceptor, config, lifecycle, dev-mode
* ✅ `r2e-security` — JWT/JWKS, AuthenticatedUser, RoleExtractor
* ✅ `r2e-events` — EventBus typé avec consumers déclaratifs
* ✅ `r2e-scheduler` — Tâches planifiées (interval, cron) avec shutdown gracieux
* ✅ `r2e-data` — Entity, QueryBuilder, Repository, pagination
* ✅ `r2e-cache` — TtlCache + CacheStore pluggable
* ✅ `r2e-rate-limit` — Rate limiting token-bucket pluggable
* ✅ `r2e-openapi` — Spec OpenAPI 3.0.3 + Swagger UI
* ✅ `r2e-utils` — Intercepteurs built-in (Logged, Timed, Cache, CacheInvalidate)
* ✅ `r2e-test` — TestApp + TestJwt
* ✅ `r2e-cli` — Scaffold et dev-mode
* ✅ `example-app` — Démo complète avec JWT, CRUD, events, scheduling, intercepteurs, rate limiting, transactions

---
