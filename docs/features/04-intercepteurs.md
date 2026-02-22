# Feature 4 — Intercepteurs

## Objectif

Fournir des attributs declaratifs pour enrichir le comportement des methodes de controller : logging, mesure de performance, cache, rate limiting, transactions, et intercepteurs custom definis par l'utilisateur.

## Architecture

Les intercepteurs reposent sur un trait generique `Interceptor<R>` avec un pattern `around` (defini dans `r2e-core/src/interceptors.rs`). Les intercepteurs built-in (`Logged`, `Timed`, `Cached`) sont des structs qui implementent ce trait. Tous les appels sont monomorphises (pas de `dyn`) → zero-cost a l'execution.

Exceptions a cette architecture :
- **`rate_limited`** — gere au niveau du handler (short-circuit avant le controller, comme `#[roles]`)
- **`transactional`** — reste en codegen pur (injection de la variable `tx` dans le body)
- **`cache_invalidate`** — codegen pur (appel apres le body)

Les intercepteurs user-defined implementent le meme trait et s'appliquent via `#[intercept(TypeName)]`.

### Pourquoi des attributs no-op ?

Tous les attributs d'intercepteur (`#[logged]`, `#[timed]`, `#[cached]`, `#[rate_limited]`, `#[transactional]`, `#[cache_invalidate]`, `#[intercept]`) sont declares dans `r2e-macros/src/lib.rs` comme des `#[proc_macro_attribute]` no-op — ils retournent leur input sans transformation. La vraie logique est dans l'attribut `#[routes]`, qui parse ces attributs depuis le flux de tokens brut du bloc `impl`.

Ces declarations no-op existent pour trois raisons :

1. **Eviter les erreurs compilateur** — sans declaration, `#[logged]` utilise hors de `#[routes]` (par erreur ou lors d'un refactoring) provoquerait `cannot find attribute "logged"`.
2. **Decouvrabilite** — les attributs apparaissent dans `cargo doc` avec leur documentation, rendant l'API explicite.
3. **Support IDE** — rust-analyzer et les autres outils offrent l'autocompletion et la documentation au survol pour les attributs enregistres.

## Vue d'ensemble

| Attribut | Effet | Prerequis |
|----------|-------|-----------|
| `#[logged]` | Log `entering`/`exiting` via trait `Interceptor` | Aucun |
| `#[logged(level = "debug")]` | Idem, niveau configurable | Aucun |
| `#[timed]` | Log le temps d'execution | Aucun |
| `#[timed(threshold = 100)]` | Log seulement si > 100ms | Aucun |
| `#[cached(ttl = N)]` | Cache le resultat pendant N secondes | Retour `axum::Json<T>` ou `T: Serialize + DeserializeOwned` |
| `#[cached(ttl = N, group = "x")]` | Cache nomme (pour invalidation) | Idem |
| `#[cached(ttl = N, key = "params")]` | Cle basee sur les parametres | Parametres impl `Debug` |
| `#[cache_invalidate("x")]` | Invalide un groupe de cache apres execution | Aucun |
| `#[rate_limited(max = N, window = S)]` | Limite globale de requetes | Aucun |
| `#[rate_limited(..., key = "user")]` | Limite par utilisateur | Champ `#[identity]` |
| `#[rate_limited(..., key = "ip")]` | Limite par adresse IP | Header `X-Forwarded-For` |
| `#[transactional]` | Transaction SQL auto-commit/rollback | Champ `pool` injecte |
| `#[transactional(pool = "read_db")]` | Transaction sur un pool specifique | Champ correspondant injecte |
| `#[intercept(Type)]` | Intercepteur custom user-defined | Le type impl `Interceptor<R>` |

### Ordre d'application

Les intercepteurs s'appliquent dans un ordre fixe, de l'exterieur vers l'interieur :

```
Niveau handler (avant le controller, dans generate_single_handler) :
  → rate_limited (si present) — short-circuit 429
  → roles (si present) — short-circuit 403

Niveau body (trait Interceptor::around, dans generate_wrapped_method) :
  → logged
  → timed
  → intercepteurs user-defined (#[intercept(...)]) dans l'ordre de declaration
  → cached

Codegen pur (wrapping inline) :
  → cache_invalidate (apres le body)
  → transactional (injection de tx)
  → corps de la methode
```

## Le trait `Interceptor<R>`

```rust
/// Contexte passe a chaque intercepteur. Copy pour capture par closures async move.
#[derive(Clone, Copy)]
pub struct InterceptorContext {
    pub method_name: &'static str,
    pub controller_name: &'static str,
}

/// Trait generique sur le type de retour R.
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
```

`InterceptorContext` est `Copy`, ce qui permet sa capture par chaque closure `async move` imbriquee sans probleme d'ownership.

## `#[logged]`

Ajoute des traces a l'entree et la sortie via le trait `Interceptor`.

```rust
#[get("/users")]
#[logged]                        // defaut: Info
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[logged(level = "debug")]       // niveau custom
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Niveaux disponibles : `trace`, `debug`, `info`, `warn`, `error`.

Logs generes (niveau info) :

```
INFO method="list" "entering"
INFO method="list" "exiting"
```

## `#[timed]`

Mesure le temps d'execution. Avec un seuil optionnel, ne log que si le temps depasse le seuil.

```rust
#[get("/users")]
#[timed]                                     // defaut: Info, pas de seuil
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[timed(level = "warn", threshold = 100)]    // seulement si > 100ms
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Log genere (sans seuil ou seuil depasse) :

```
INFO method="list" "elapsed_ms=3"
```

### Combinaison `#[logged]` + `#[timed]`

```rust
#[get("/users")]
#[logged(level = "debug")]
#[timed(threshold = 50)]
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Logs (si la requete prend plus de 50ms) :

```
DEBUG method="list" "entering"
INFO  method="list" "elapsed_ms=73"
DEBUG method="list" "exiting"
```

## `#[cached]`

Cache le resultat de la methode. Le cache utilise le trait `Interceptor<axum::Json<T>>` ou `T: Serialize + DeserializeOwned`.

### Syntaxes

```rust
#[cached(ttl = 30)]                          // cache anonyme, cle par defaut
#[cached(ttl = 30, group = "users")]         // cache nomme (pour invalidation)
#[cached(ttl = 30, key = "params")]          // cle basee sur les parametres
#[cached(ttl = 30, key = "user")]            // cle par utilisateur (identity.sub)
#[cached(ttl = 30, key = "user_params")]     // combinaison user + params
```

### Contraintes

- Le type de retour **doit** etre `axum::Json<T>` ou `T: Serialize + DeserializeOwned` (pas `Result<Json<T>, HttpError>`)
- Le cache serialise/deserialise en JSON string via `serde_json`
- Pour `key = "params"`, les parametres de la methode doivent implementer `Debug`
- Pour `key = "user"` ou `key = "user_params"`, le controller doit avoir un champ `#[identity]`

### Cache groups et invalidation

```rust
#[get("/users")]
#[cached(ttl = 30, group = "users")]
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[post("/users")]
#[cache_invalidate("users")]
async fn create(&self, ...) -> axum::Json<User> { ... }
```

Le `CacheRegistry` (statique global dans `r2e-core/src/cache.rs`) maintient un registre de caches nommes :
- `get_or_create(group, ttl)` — retourne le cache du groupe (le cree au premier appel)
- `invalidate(group)` — vide le cache du groupe

**Note** : le TTL est determine par le premier appel a `get_or_create`. Si deux methodes referent au meme groupe avec des TTL differents, le premier a s'executer impose le TTL.

### Fonctionnement interne

```
Requete → Interceptor::around(&cached, ctx, next)
            ├── cache.get(cle)
            │     ├── Hit → deserialiser → retourner Json<T>
            │     └── Deserialization echouee → cache.remove(cle) → fallthrough
            └── Miss → next().await → serialiser → cache.insert(cle) → retourner
```

## `#[rate_limited]`

Limite le nombre de requetes. Gere au **niveau handler** (short-circuit avant le controller).

### Syntaxes

```rust
#[rate_limited(max = 5, window = 60)]                   // global
#[rate_limited(max = 5, window = 60, key = "user")]      // par utilisateur
#[rate_limited(max = 5, window = 60, key = "ip")]        // par adresse IP
```

### Strategies de cle

| Cle | Code genere | Prerequis |
|-----|-------------|-----------|
| `"global"` (defaut) | `format!("{}:global", fn_name)` | Aucun |
| `"user"` | `format!("{}:user:{}", fn_name, identity.sub)` | Champ `#[identity]` |
| `"ip"` | `format!("{}:ip:{}", fn_name, ip)` | Header `X-Forwarded-For` |

Pour `key = "ip"`, l'IP est extraite depuis le header `X-Forwarded-For` (premier element, trimme). Fallback : `"unknown"`.

### Contraintes

- Le handler genere retourne `axum::response::Response` (comme `#[roles]`) pour permettre le short-circuit 429
- Le type de retour de la methode n'a **plus besoin** d'etre `Result<T, HttpError>` — n'importe quel type `IntoResponse` fonctionne
- Le rate limiter est un `static OnceLock<RateLimiter<String>>` par handler

### Reponse en cas de depassement

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/json

{"error": "Rate limit exceeded"}
```

### Fonctionnement interne

Le `RateLimiter<K>` utilise un algorithme de **token bucket** :
- Chaque cle dispose d'un seau de `max` tokens
- Les tokens se rechargent lineairement sur la `window`
- Chaque requete consomme 1 token
- Si le seau est vide → rejet 429

## `#[transactional]`

Enveloppe le corps de la methode dans une transaction SQL.

```rust
#[post("/users/db")]
#[transactional]                             // defaut: self.pool
async fn create_in_db(&self, ...) -> Result<axum::Json<User>, r2e_core::HttpError> {
    sqlx::query("INSERT ...").execute(&mut *tx).await?;
    Ok(axum::Json(user))
}

#[transactional(pool = "read_db")]           // pool specifique
async fn read_data(&self, ...) -> Result<...> { ... }
```

### Contraintes

- Le controller doit avoir un champ `#[inject]` pour le pool specifie (defaut: `pool`)
- Le corps peut utiliser `tx` (variable injectee par la macro, de type `Transaction`)
- Le type de retour **doit** etre `Result<T, HttpError>`

## `#[intercept(Type)]` — Intercepteurs user-defined

Les utilisateurs peuvent creer leurs propres intercepteurs en implementant le trait `Interceptor<R>` :

```rust
pub struct AuditLog;

impl<R: Send> r2e_core::Interceptor<R> for AuditLog {
    fn around<F, Fut>(
        &self,
        ctx: r2e_core::InterceptorContext,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move {
            tracing::info!(method = ctx.method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "audit: done");
            result
        }
    }
}
```

Application :

```rust
#[get("/users/audited")]
#[logged]
#[intercept(AuditLog)]
async fn audited_list(&self) -> axum::Json<Vec<User>> { ... }
```

### Contraintes

- Le type passe a `#[intercept(...)]` doit etre constructible comme expression de chemin (struct unitaire ou constante). Pas de syntaxe d'appel (`#[intercept(Foo::new())]` ne fonctionne pas).
- L'intercepteur est generique sur `R` (ou contraint sur un type specifique si besoin).

## Exemple complet

```rust
use std::future::Future;
use r2e_core::prelude::*;

/// Intercepteur custom
pub struct AuditLog;

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F)
        -> impl Future<Output = R> + Send
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send,
    {
        async move {
            tracing::info!(method = ctx.method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "audit: done");
            result
        }
    }
}

#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject]
    user_service: UserService,

    #[inject]
    pool: sqlx::SqlitePool,

    #[identity]
    user: AuthenticatedUser,
}

#[routes]
impl UserController {
    // Logged debug + timed avec seuil
    #[get("/")]
    #[logged(level = "debug")]
    #[timed(threshold = 50)]
    async fn list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }

    // Cache groupe + invalidation
    #[get("/cached")]
    #[cached(ttl = 30, group = "users")]
    #[timed]
    async fn cached_list(&self) -> axum::Json<serde_json::Value> {
        let users = self.user_service.list().await;
        axum::Json(serde_json::to_value(users).unwrap())
    }

    #[post("/")]
    #[cache_invalidate("users")]
    async fn create(&self, axum::Json(body): axum::Json<CreateUserRequest>) -> axum::Json<User> {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Rate limit par user
    #[post("/rate-limited")]
    #[rate_limited(max = 5, window = 60, key = "user")]
    async fn create_rate_limited(&self, axum::Json(body): axum::Json<CreateUserRequest>)
        -> axum::Json<User>
    {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Transaction
    #[post("/db")]
    #[transactional]
    async fn create_in_db(&self, axum::Json(body): axum::Json<CreateUserRequest>)
        -> Result<axum::Json<User>, r2e_core::HttpError>
    {
        sqlx::query("INSERT INTO users ...").execute(&mut *tx).await?;
        Ok(axum::Json(user))
    }

    // Intercepteur custom
    #[get("/audited")]
    #[logged]
    #[intercept(AuditLog)]
    async fn audited_list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }
}
```

## Critere de validation

```bash
# Cached avec groupe — deux appels rapides, le second vient du cache
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached

# Cache invalidation — create vide le cache
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"New","email":"new@example.com"}'
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached
# → contient le nouvel utilisateur

# Rate limited per-user — apres 5 requetes, la 6e retourne 429
for i in $(seq 1 6); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    -X POST http://localhost:3000/users/rate-limited \
    -H "Authorization: Bearer <token>" \
    -H "Content-Type: application/json" \
    -d '{"name":"Test","email":"test@example.com"}'
done
# → 200 200 200 200 200 429

# Deux users distincts ont des compteurs independants (key = "user")

# Intercepteur custom — log d'audit visible dans la sortie
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/audited
# → logs: audit: entering / audit: done
```
