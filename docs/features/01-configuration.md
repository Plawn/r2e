# Feature 1 — Configuration

## Objectif

Fournir un systeme de configuration type, charge depuis des fichiers YAML et des variables d'environnement, avec support de profils (`dev`, `prod`, `test`).

## Concepts cles

### QuarlusConfig

`QuarlusConfig` est le conteneur central de configuration. Il stocke les valeurs sous forme de cles separees par des points (ex: `app.database.url`).

### Ordre de resolution

1. `application.yaml` — configuration de base
2. `application-{profile}.yaml` — surcharge par profil
3. Variables d'environnement — surcharge finale (convention : `APP_DATABASE_URL` ↔ `app.database.url`)

### Profil actif

Determine par : variable d'environnement `QUARLUS_PROFILE` > argument de `load()` > defaut `"dev"`.

## Utilisation

### 1. Fichier de configuration

Creer un fichier `application.yaml` a la racine du workspace :

```yaml
app:
  name: "Mon Application"
  greeting: "Bienvenue !"
  version: "0.1.0"

database:
  url: "sqlite::memory:"
  pool_size: 10
```

### 2. Charger la configuration

```rust
use quarlus_core::config::{QuarlusConfig, ConfigValue};

let config = QuarlusConfig::load("dev").unwrap_or_else(|_| QuarlusConfig::empty());
```

`load()` reussit meme si le fichier YAML est absent (les variables d'environnement sont toujours overlayees). Pour garantir la presence de cles requises, verifier et definir des valeurs par defaut :

```rust
let mut config = QuarlusConfig::load("dev").unwrap_or_else(|_| QuarlusConfig::empty());

if config.get::<String>("app.name").is_err() {
    config.set("app.name", ConfigValue::String("Default App".into()));
}
```

### 3. Lire des valeurs

```rust
// Lecture typee
let name: String = config.get("app.name").unwrap();
let pool_size: i64 = config.get("database.pool_size").unwrap();
let debug: bool = config.get("app.debug").unwrap_or(false);

// Avec valeur par defaut
let timeout: i64 = config.get_or("app.timeout", 30);
```

### Types supportes

| Type | ConfigValue | Conversion |
|------|------------|------------|
| `String` | Tous les types | `.to_string()` |
| `i64` | `Integer`, `String` (parsable) | Direct ou parse |
| `f64` | `Float`, `Integer`, `String` | Direct ou parse |
| `bool` | `Bool`, `String` (`"true"/"false"/"1"/"0"/"yes"/"no"`) | Direct ou parse |
| `Option<T>` | `Null` → `None`, autre → `Some(T)` | Recursif |

### 4. Injection dans un controller via `#[config]`

Le champ `#[config("cle")]` dans un controller injecte automatiquement la valeur depuis la configuration au moment de la requete :

```rust
#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct MyController {
    #[config("app.greeting")]
    greeting: String,

    #[config("app.name")]
    app_name: String,
}

#[quarlus_macros::routes]
impl MyController {
    #[get("/greeting")]
    async fn greeting(&self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "greeting": self.greeting,
            "app": self.app_name,
        }))
    }
}
```

### Prerequis pour `#[config]`

Le type d'etat (`Services`) doit implementer `FromRef<Services>` pour `QuarlusConfig` :

```rust
impl axum::extract::FromRef<Services> for QuarlusConfig {
    fn from_ref(state: &Services) -> Self {
        state.config.clone()
    }
}
```

### 5. Enregistrer la config dans l'AppBuilder

```rust
AppBuilder::new()
    .with_state(services)
    .with_config(config)
    // ...
```

## Variables d'environnement

Les variables d'environnement surchargent toute valeur YAML. La convention de nommage est :

```
Cle YAML          →  Variable d'environnement
app.database.url  →  APP_DATABASE_URL
app.name          →  APP_NAME
```

La conversion est : minuscules, remplacement de `_` par `.`.

## Tests

En test, utiliser `QuarlusConfig::empty()` pour creer une configuration vide et definir les valeurs programmatiquement :

```rust
let mut config = QuarlusConfig::empty();
config.set("app.name", ConfigValue::String("Test App".into()));
config.set("app.greeting", ConfigValue::String("Hello from tests!".into()));
```

## Critere de validation

```bash
curl -H "Authorization: Bearer <token>" http://localhost:3000/greeting
# → {"greeting":"Bienvenue !"}

curl http://localhost:3000/config
# → {"app_name":"Mon Application","app_version":"0.1.0"}
```
