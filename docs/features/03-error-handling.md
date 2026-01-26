# Feature 3 — Gestion d'erreurs

## Objectif

Fournir un systeme d'erreurs structure qui convertit automatiquement les erreurs en reponses HTTP JSON coherentes, avec support pour les erreurs custom et la capture des panics.

## Concepts cles

### AppError

`AppError` est l'enum centrale qui represente toutes les erreurs applicatives. Chaque variant correspond a un code HTTP specifique.

### map_error!

Macro pour generer des implementations `From<E> for AppError` en une ligne.

### Catch panic

Layer Tower qui capture les panics dans les handlers et les convertit en reponses 500.

## Variants d'AppError

| Variant | Code HTTP | Usage |
|---------|-----------|-------|
| `NotFound(String)` | 404 | Ressource introuvable |
| `Unauthorized(String)` | 401 | Authentification requise/invalide |
| `Forbidden(String)` | 403 | Droits insuffisants |
| `BadRequest(String)` | 400 | Requete mal formee |
| `Internal(String)` | 500 | Erreur serveur |
| `Validation(ValidationErrorResponse)` | 400 | Echec de validation (feature `validation`) |
| `Custom { status, body }` | Custom | Code HTTP et body JSON arbitraires |

## Utilisation

### 1. Retourner des erreurs standard

```rust
#[get("/users/{id}")]
async fn get_by_id(
    &self,
    Path(id): Path<u64>,
) -> Result<axum::Json<User>, quarlus_core::AppError> {
    match self.user_service.get_by_id(id).await {
        Some(user) => Ok(axum::Json(user)),
        None => Err(quarlus_core::AppError::NotFound("User not found".into())),
    }
}
```

Reponse generee :

```http
HTTP/1.1 404 Not Found
Content-Type: application/json

{"error": "User not found"}
```

### 2. Erreurs custom avec code HTTP arbitraire

Le variant `Custom` permet de retourner n'importe quel code HTTP avec un body JSON libre :

```rust
#[get("/error/custom")]
async fn custom_error(&self) -> Result<axum::Json<()>, quarlus_core::AppError> {
    Err(quarlus_core::AppError::Custom {
        status: axum::http::StatusCode::from_u16(418).unwrap(),
        body: serde_json::json!({
            "error": "I'm a teapot",
            "code": 418
        }),
    })
}
```

Reponse :

```http
HTTP/1.1 418 I'm a Teapot
Content-Type: application/json

{"error": "I'm a teapot", "code": 418}
```

### 3. Conversions automatiques avec `From`

`AppError` implemente `From` pour les types d'erreur courants, ce qui permet l'usage de `?` :

```rust
// Inclus par defaut
impl From<std::io::Error> for AppError { ... }

// Inclus avec le feature flag "sqlx"
impl From<sqlx::Error> for AppError { ... }
```

### 4. Macro `map_error!`

Pour ajouter des conversions supplementaires dans votre code applicatif :

```rust
quarlus_core::map_error! {
    serde_json::Error => Internal,
    reqwest::Error => Internal,
}
```

Cela genere :

```rust
impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}
```

**Note** : `map_error!` genere des `impl From` — les deux types (erreur source et `AppError`) doivent respecter la regle de coherence (orphan rule). Utilisez-le uniquement pour des types d'erreurs definis dans votre crate, ou dans la crate ou `AppError` est defini.

### 5. Catch panic (layer Tower)

Activer la capture des panics dans l'`AppBuilder` :

```rust
AppBuilder::new()
    .with_state(services)
    .with_error_handling()  // Active catch_panic_layer
    // ...
```

Si un handler panique, au lieu d'un crash, le client recoit :

```http
HTTP/1.1 500 Internal Server Error
Content-Type: application/json

{"error": "Internal server error"}
```

## Combinaison avec d'autres features

### Avec Validation (#2)

Quand le feature flag `validation` est actif, `AppError::Validation` fournit une reponse 400 structuree avec le detail par champ :

```json
{
    "error": "Validation failed",
    "details": [
        {"field": "email", "message": "...", "code": "email"}
    ]
}
```

### Avec `#[transactional]` (#4)

Les erreurs dans un bloc transactionnel provoquent un rollback automatique de la transaction :

```rust
#[post("/users/db")]
#[transactional]
async fn create_in_db(&self, ...) -> Result<Json<User>, AppError> {
    // Si une erreur survient ici, tx.rollback() est appele automatiquement
    sqlx::query("INSERT INTO users ...").execute(&mut *tx).await?;
    Ok(...)
}
```

## Critere de validation

```bash
# Erreur 404
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/999
# → {"error":"User not found"}

# Erreur 418 custom
curl -H "Authorization: Bearer <token>" http://localhost:3000/error/custom
# → {"error":"I'm a teapot","code":418}
```
