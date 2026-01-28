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
 ├─ quarlus-macros/       # Proc-macros (controller, inject, routes…)
 ├─ quarlus-core/         # Runtime minimal + Axum glue + AppBuilder + config + guards + intercepteurs
 ├─ quarlus-security/     # JWT / Identity / OIDC / JWKS
 ├─ quarlus-events/       # EventBus pub/sub typé
 ├─ quarlus-scheduler/    # Tâches planifiées (interval, cron, delay)
 ├─ quarlus-data/         # Entity, QueryBuilder, Repository, Pageable/Page
 ├─ quarlus-cache/        # TtlCache, CacheStore trait, InMemoryStore
 ├─ quarlus-rate-limit/   # RateLimiter token-bucket, RateLimitRegistry
 ├─ quarlus-openapi/      # Génération OpenAPI 3.0.3 + Swagger UI
 ├─ quarlus-utils/        # Intercepteurs built-in (Logged, Timed, Cache, CacheInvalidate)
 ├─ quarlus-test/         # TestApp, TestJwt pour tests d'intégration
 ├─ quarlus-cli/          # CLI : quarlus new/add/dev/generate
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

*Toutes implémentées :*

* ✅ `#[roles("admin")]` — guard de rôles (quarlus-security + quarlus-macros)
* ✅ `#[transactional]` — wrapping SQL transaction automatique (quarlus-macros)
* ✅ `#[config("key")]` — injection de configuration (quarlus-core + quarlus-macros)
* ✅ OpenAPI auto — génération spec 3.0.3 + Swagger UI (quarlus-openapi)
* ✅ Dev mode / hot reload — endpoints `/__quarlus_dev/*` (quarlus-core)

*Ajouts supplémentaires réalisés :*

* ✅ `#[rate_limited]` — rate limiting par token bucket (quarlus-rate-limit)
* ✅ `#[intercept(...)]` — intercepteurs (Logged, Timed, Cache, CacheInvalidate + custom)
* ✅ `#[guard(...)]` — guards custom (quarlus-core)
* ✅ `#[consumer(bus = "...")]` — consommateurs d'événements (quarlus-events)
* ✅ `#[scheduled(every/cron)]` — tâches planifiées (quarlus-scheduler)
* ✅ `#[middleware(...)]` — middleware Tower par route
* ✅ Data/Repository — Entity, QueryBuilder, Pageable, Page (quarlus-data)
* ✅ Cache pluggable — CacheStore trait + InMemoryStore (quarlus-cache)
* ✅ Test helpers — TestApp, TestJwt (quarlus-test)
* ✅ CLI — quarlus new/add/dev/generate (quarlus-cli)
* ✅ Lifecycle hooks — on_start / on_stop (quarlus-core)
* ✅ Validation — Validated<T> extractor (quarlus-core, feature-gated)

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

* ✅ `quarlus-macros` — `#[derive(Controller)]` + `#[routes]` avec tous les attributs
* ✅ `quarlus-core` — AppBuilder, Controller, Guard, Interceptor, config, lifecycle, dev-mode
* ✅ `quarlus-security` — JWT/JWKS, AuthenticatedUser, RoleExtractor
* ✅ `quarlus-events` — EventBus typé avec consumers déclaratifs
* ✅ `quarlus-scheduler` — Tâches planifiées (interval, cron) avec shutdown gracieux
* ✅ `quarlus-data` — Entity, QueryBuilder, Repository, pagination
* ✅ `quarlus-cache` — TtlCache + CacheStore pluggable
* ✅ `quarlus-rate-limit` — Rate limiting token-bucket pluggable
* ✅ `quarlus-openapi` — Spec OpenAPI 3.0.3 + Swagger UI
* ✅ `quarlus-utils` — Intercepteurs built-in (Logged, Timed, Cache, CacheInvalidate)
* ✅ `quarlus-test` — TestApp + TestJwt
* ✅ `quarlus-cli` — Scaffold et dev-mode
* ✅ `example-app` — Démo complète avec JWT, CRUD, events, scheduling, intercepteurs, rate limiting, transactions

---
