# Feature 4 — Intercepteurs

## Objectif

Fournir des attributs declaratifs pour enrichir le comportement des methodes de controller : logging, mesure de performance, cache, rate limiting, et transactions.

## Vue d'ensemble

Les intercepteurs sont des attributs appliques aux methodes de route dans un bloc `controller!`. La macro genere le code d'enrobage correspondant au compile-time.

| Attribut | Effet | Prerequis |
|----------|-------|-----------|
| `#[logged]` | Log `entering`/`exiting` via `tracing::info` | Aucun |
| `#[timed]` | Log le temps d'execution en millisecondes | Aucun |
| `#[cached(ttl = N)]` | Cache le resultat pendant N secondes | Retour `axum::Json<serde_json::Value>` |
| `#[rate_limited(max = N, window = S)]` | Limite a N requetes par fenetre de S secondes | Retour `Result<T, AppError>` |
| `#[transactional]` | Enveloppe dans une transaction SQL | Champ `pool: sqlx::SqlitePool` injecte |

### Ordre d'application

Les intercepteurs s'appliquent dans un ordre fixe (de l'exterieur vers l'interieur) :

```
logged → timed → rate_limited → cached → transactional → corps de la methode
```

Cela signifie que le timing inclut le rate-limiting et le cache, et que le logging enveloppe tout.

## `#[logged]`

Ajoute des traces `tracing::info` a l'entree et la sortie de la methode.

```rust
#[get("/users")]
#[logged]
async fn list(&self) -> axum::Json<Vec<User>> {
    // ...
}
```

Logs generes :

```
INFO method="list" "entering"
INFO method="list" "exiting"
```

## `#[timed]`

Mesure le temps d'execution et le log en millisecondes.

```rust
#[get("/users")]
#[timed]
async fn list(&self) -> axum::Json<Vec<User>> {
    // ...
}
```

Log genere :

```
INFO method="list" elapsed_ms=3 "method execution time"
```

### Combinaison `#[logged]` + `#[timed]`

Les deux peuvent etre combines :

```rust
#[get("/users")]
#[logged]
#[timed]
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Logs :

```
INFO method="list" "entering"
INFO method="list" elapsed_ms=3 "method execution time"
INFO method="list" "exiting"
```

## `#[cached(ttl = N)]`

Cache le resultat de la methode dans un `TtlCache` statique. Les appels suivants dans la fenetre TTL retournent la valeur cachee sans executer le corps.

```rust
#[get("/users/cached")]
#[cached(ttl = 30)]
async fn cached_list(&self) -> axum::Json<serde_json::Value> {
    let users = self.user_service.list().await;
    axum::Json(serde_json::to_value(users).unwrap())
}
```

### Contraintes

- Le type de retour **doit** etre `axum::Json<serde_json::Value>` (le cache serialise/deserialise en JSON string)
- Le cache est **global** (statique) — toutes les requetes partagent le meme cache
- La cle de cache est basee sur le nom de la methode (pas sur les parametres de requete)

### Fonctionnement interne

```
Requete → cache.get(cle)
           ├── Hit → retourner la valeur cachee (pas d'execution du corps)
           └── Miss → executer le corps → cache.insert(cle, resultat) → retourner
```

Le `TtlCache<K, V>` sous-jacent utilise `DashMap` pour la concurrence et evicte lazily les entrees expirees.

## `#[rate_limited(max = N, window = S)]`

Limite le nombre de requetes a `max` par fenetre de `window` secondes. Retourne 429 (Too Many Requests) si la limite est depassee.

```rust
#[post("/users/rate-limited")]
#[rate_limited(max = 5, window = 60)]
async fn create_rate_limited(
    &self,
    Validated(body): Validated<CreateUserRequest>,
) -> Result<axum::Json<User>, quarlus_core::AppError> {
    let user = self.user_service.create(body.name, body.email).await;
    Ok(axum::Json(user))
}
```

### Contraintes

- Le type de retour **doit** etre `Result<T, quarlus_core::AppError>` (le rate limiter utilise `return Err(...)`)
- Le rate limiter est **global** (statique) — le compteur est partage entre toutes les requetes

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

Enveloppe le corps de la methode dans une transaction SQL. Commit automatique en cas de succes, pas de commit en cas d'erreur (la transaction est droppee et rollbackee).

```rust
#[post("/users/db")]
#[transactional]
async fn create_in_db(
    &self,
    axum::Json(body): axum::Json<CreateUserRequest>,
) -> Result<axum::Json<User>, quarlus_core::AppError> {
    sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
        .bind(&body.name)
        .bind(&body.email)
        .execute(&mut *tx)  // `tx` est injecte par la macro
        .await?;
    // ...
    Ok(axum::Json(user))
}
```

### Contraintes

- Le controller doit avoir un champ `#[inject] pool: sqlx::SqlitePool`
- Le corps peut utiliser `tx` (variable injectee par la macro, de type `Transaction`)
- Le type de retour **doit** etre `Result<T, AppError>`

### Fonctionnement genere

```rust
// Code genere (simplifie)
{
    let mut tx = self.pool.begin().await?;
    let result = { /* corps original */ };
    match result {
        Ok(val) => { tx.commit().await?; Ok(val) }
        Err(err) => Err(err)  // tx est droppee → rollback automatique
    }
}
```

## Exemple complet

Un controller combinant tous les intercepteurs :

```rust
quarlus_macros::controller! {
    impl UserController for Services {
        #[inject]
        user_service: UserService,

        #[inject]
        pool: sqlx::SqlitePool,

        #[get("/users")]
        #[logged]
        #[timed]
        async fn list(&self) -> axum::Json<Vec<User>> {
            axum::Json(self.user_service.list().await)
        }

        #[get("/users/cached")]
        #[cached(ttl = 30)]
        #[timed]
        async fn cached_list(&self) -> axum::Json<serde_json::Value> {
            let users = self.user_service.list().await;
            axum::Json(serde_json::to_value(users).unwrap())
        }

        #[post("/users/rate-limited")]
        #[rate_limited(max = 5, window = 60)]
        async fn create_rate_limited(
            &self,
            Validated(body): Validated<CreateUserRequest>,
        ) -> Result<axum::Json<User>, quarlus_core::AppError> {
            Ok(axum::Json(self.user_service.create(body.name, body.email).await))
        }

        #[post("/users/db")]
        #[transactional]
        async fn create_in_db(
            &self,
            axum::Json(body): axum::Json<CreateUserRequest>,
        ) -> Result<axum::Json<User>, quarlus_core::AppError> {
            sqlx::query("INSERT INTO users ...").execute(&mut *tx).await?;
            Ok(axum::Json(user))
        }
    }
}
```

## Critere de validation

```bash
# Cached — deux appels rapides, le second vient du cache
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached

# Rate limited — apres 5 requetes, la 6e retourne 429
for i in $(seq 1 6); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    -X POST http://localhost:3000/users/rate-limited \
    -H "Authorization: Bearer <token>" \
    -H "Content-Type: application/json" \
    -d '{"name":"Test","email":"test@example.com"}'
done
# → 200 200 200 200 200 429
```
