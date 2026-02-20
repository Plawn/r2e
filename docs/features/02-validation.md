# Feature 2 — Validation

## Objectif

Valider automatiquement les corps de requete JSON avec des regles declaratives, et retourner des reponses 400 structurees en cas d'echec.

## Concepts cles

### Validation automatique

R2E valide automatiquement les parametres de handler qui derivent `garde::Validate`. Il suffit de deriver `Validate` sur le type et d'utiliser `Json<T>` — la validation se fait de maniere transparente dans le code genere.

### Crate garde

R2E utilise la crate `garde` pour declarer les regles de validation sur les structs. Garde offre une verification compile-time des regles et un systeme de contexte type.

## Utilisation

### 1. Definir un modele avec des regles de validation

```rust
use serde::Deserialize;
use garde::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,

    #[garde(email)]
    pub email: String,
}
```

### Regles disponibles (crate `garde`)

| Regle | Attribut | Exemple |
|-------|---------|---------|
| Longueur | `#[garde(length(min=1, max=100))]` | Chaines |
| Email | `#[garde(email)]` | Format email |
| URL | `#[garde(url)]` | Format URL |
| Range | `#[garde(range(min=0, max=1000))]` | Nombres |
| Pattern | `#[garde(pattern("regex"))]` | Patterns custom |
| Custom | `#[garde(custom(my_fn))]` | Logique arbitraire |
| Skip | `#[garde(skip)]` | Ne pas valider ce champ |

### 2. Utiliser dans un handler

Utiliser `Json<T>` normalement — la validation est automatique :

```rust
use r2e::prelude::*;

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
        Json(body): Json<CreateUserRequest>,
    ) -> Json<User> {
        // `body` est garanti valide ici
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
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
            "message": "not a valid email address",
            "code": "validation"
        },
        {
            "field": "name",
            "message": "length is lower than 1",
            "code": "validation"
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

## Params — extraction agregee de parametres

`#[derive(Params)]` permet de regrouper path, query et header dans un seul struct (equivalent de `@BeanParam` en JAX-RS). Combine avec `garde::Validate`, tous les parametres sont extraits **et** valides en une seule etape.

### Definition

```rust
use r2e::prelude::*;
use garde::Validate;

#[derive(Params, Validate)]
pub struct GetUserParams {
    #[path]
    #[garde(skip)]
    pub id: u64,

    #[query]
    #[garde(range(min = 1))]
    pub page: Option<u32>,

    #[header("X-Tenant-Id")]
    #[garde(length(min = 1))]
    pub tenant_id: String,
}
```

### Attributs disponibles

| Attribut | Source | Nom par defaut |
|----------|--------|---------------|
| `#[path]` | Segments du path | Nom du champ |
| `#[path(name = "userId")]` | Segments du path | Nom custom |
| `#[query]` | Query string | Nom du champ |
| `#[query(name = "q")]` | Query string | Nom custom |
| `#[header("X-Custom")]` | Headers HTTP | Nom explicite (obligatoire) |

- `Option<T>` → parametre optionnel (absent = `None`)
- `T` non-Option → parametre requis (absent = 400 Bad Request)
- Conversion via `FromStr` pour les types non-String

### Utilisation dans un handler

```rust
#[routes]
impl UserController {
    #[get("/{id}")]
    async fn get_user(&self, params: GetUserParams) -> Json<User> {
        // params.id, params.page, params.tenant_id extraits et valides
        let user = self.user_service.find(params.id).await;
        Json(user)
    }
}
```

## Fonctionnement interne

Le code genere par `#[routes]` utilise un mecanisme d'autoref specialization :

1. Deserialisation via `Json<T>` (Axum standard)
2. Validation automatique via `__AutoValidator` — si le type derive `Validate`, la validation est executee ; sinon, c'est un no-op (zero overhead)
3. Si echec → reponse 400 avec le detail des erreurs par champ

Les types sans `#[derive(Validate)]` fonctionnent normalement — aucune validation n'est executee.

## Dependencies

```toml
[dependencies]
r2e = "0.1"
garde = { version = "0.22", features = ["derive", "email"] }
```

La validation est toujours disponible — plus besoin de feature flag.

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
