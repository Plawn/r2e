# Feature 2 — Validation

## Objectif

Valider automatiquement les corps de requete JSON avec des regles declaratives, et retourner des reponses 400 structurees en cas d'echec.

## Concepts cles

### Validated<T>

`Validated<T>` est un extracteur Axum qui remplace `Json<T>`. Il deserialise le JSON **puis** applique les regles `validator::Validate`. Si la validation echoue, une reponse 400 est retournee avec le detail des erreurs par champ.

### Crate validator

R2E utilise la crate `validator` (derive) pour declarer les regles de validation sur les structs.

## Utilisation

### 1. Definir un modele avec des regles de validation

```rust
use serde::Deserialize;
use validator::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(email)]
    pub email: String,
}
```

### Regles disponibles (crate `validator`)

| Regle | Attribut | Exemple |
|-------|---------|---------|
| Longueur | `#[validate(length(min=1, max=100))]` | Chaines |
| Email | `#[validate(email)]` | Format email |
| URL | `#[validate(url)]` | Format URL |
| Range | `#[validate(range(min=0, max=1000))]` | Nombres |
| Regex | `#[validate(regex(path = "RE"))]` | Patterns custom |
| Custom | `#[validate(custom(function = "fn"))]` | Logique arbitraire |

### 2. Utiliser dans un handler

Remplacer `axum::Json<T>` par `r2e_core::validation::Validated<T>` :

```rust
use r2e_core::prelude::*;

#[derive(Controller)]
#[controller(state = Services)]
pub struct UserController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl UserController {
    #[post("/users")]
    async fn create(
        &self,
        Validated(body): Validated<CreateUserRequest>,
    ) -> axum::Json<User> {
        // `body` est garanti valide ici
        let user = self.user_service.create(body.name, body.email).await;
        axum::Json(user)
    }
}
```

### 3. Reponse en cas d'erreur de validation

Si la validation echoue, R2E retourne automatiquement :

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
    "error": "Validation failed",
    "details": [
        {
            "field": "email",
            "message": "Validation failed for field 'email'",
            "code": "email"
        },
        {
            "field": "name",
            "message": "Validation failed for field 'name'",
            "code": "length"
        }
    ]
}
```

### 4. Reponse en cas de JSON invalide

Si le corps n'est pas du JSON valide (erreur de deserialisation), une 400 standard est retournee :

```json
{
    "error": "Failed to deserialize the JSON body ..."
}
```

## Fonctionnement interne

L'extracteur `Validated<T>` implemente `FromRequest` en deux etapes :

1. Deserialisation via `axum::Json<T>` — si echec → `AppError::BadRequest`
2. Validation via `T::validate()` — si echec → `AppError::Validation` avec la liste des `FieldError`

```rust
// Simplifie
impl<T: DeserializeOwned + Validate, S: Send + Sync> FromRequest<S> for Validated<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        value.validate()?;
        Ok(Validated(value))
    }
}
```

## Dependencies

```toml
[dependencies]
r2e-core = { path = "../r2e-core", features = ["validation"] }
validator = { version = "0.18", features = ["derive"] }
```

Le feature flag `validation` active le variant `AppError::Validation` et le module `r2e_core::validation`.

## Critere de validation

```bash
# Requete valide → 200
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@example.com"}'

# Email invalide → 400
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"not-an-email"}'

# Nom vide → 400
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"","email":"alice@example.com"}'
```
