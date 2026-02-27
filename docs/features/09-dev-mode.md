# Feature 9 — Mode developpement

## Objectif

Fournir des endpoints de diagnostic pour le developpement, permettant aux outils et scripts de detecter l'etat du serveur et ses redemarrages (utile pour le hot-reload).

## Concepts cles

### Endpoints de dev

Deux endpoints sont exposes sous le prefixe `/__r2e_dev/` :

| Endpoint | Reponse | Usage |
|----------|---------|-------|
| `GET /__r2e_dev/status` | `"dev"` (texte brut) | Verifier que le serveur tourne en mode dev |
| `GET /__r2e_dev/ping` | JSON avec `boot_time` et `status` | Detecter les redemarrages |

### Boot time

Le `boot_time` est un timestamp (millisecondes depuis l'epoch Unix) capture une seule fois au demarrage du processus via `OnceLock`. Quand un outil detecte un changement de `boot_time`, cela signifie que le serveur a redemarrage.

## Utilisation

### 1. Activer le mode dev dans l'AppBuilder

```rust
AppBuilder::new()
    .with_state(services)
    .with_dev_reload()  // Active les endpoints /__r2e_dev/*
    // ...
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### 2. Endpoints disponibles

#### Status

```bash
curl http://localhost:3000/__r2e_dev/status
# → dev
```

Retourne la chaine `"dev"` en texte brut. Permet a un script de savoir que le serveur est en mode developpement.

#### Ping

```bash
curl http://localhost:3000/__r2e_dev/ping
# → {"boot_time":1706123456789,"status":"ok"}
```

Retourne un JSON avec :
- `boot_time` : timestamp du demarrage du processus (ms depuis epoch)
- `status` : toujours `"ok"`

## Hot-reload Subsecond (recommande)

R2E supporte le **hot-patching Subsecond** via Dioxus 0.7. Au lieu de tuer et relancer le serveur, Subsecond recompile uniquement le code modifie en tant que bibliotheque dynamique et le patche dans le processus en cours — typiquement en moins de 500ms.

### Configuration

1. Installer le CLI Dioxus : `cargo install dioxus-cli`
2. Ajouter le feature `dev-reload` a votre app :

```toml
[features]
dev-reload = ["r2e/dev-reload"]
```

3. Structurer votre app avec le pattern setup/serveur :

```rust
#[derive(Clone)]
struct AppEnv {
    pool: PgPool,
    config: R2eConfig,
}

async fn setup() -> AppEnv {
    // execute une seule fois, persiste entre les hot-patches
    let pool = PgPool::connect("...").await.unwrap();
    AppEnv { pool, config: R2eConfig::load("dev").unwrap() }
}

#[r2e::main]
async fn main(env: AppEnv) {
    // ce body est hot-patche a chaque changement de code
    AppBuilder::new()
        .provide(env.pool)
        .build_state::<MyState, _, _>().await
        .serve("0.0.0.0:3000").await.unwrap();
}
```

La macro `#[r2e::main]` detecte automatiquement le parametre et genere deux chemins de code gates par `#[cfg]` : execution normale et hot-patching Subsecond.

4. Lancer avec : `r2e dev`

### Fonctionnement

```
Changement de code source
    → dx detecte le changement
    → recompile UNIQUEMENT la closure serveur en bibliotheque dynamique
    → la patche dans le processus en cours (etat du setup preserve)
    → ~200-500ms de delai
```

### Polling legacy (plugin DevReload)

Le plugin `DevReload` expose `/__r2e_dev/ping` pour la detection de redemarrage. C'est toujours disponible pour les outils qui poll les redemarrages du serveur.

### Utilisation avec le CLI R2E

```bash
r2e dev
r2e dev --port 8080
r2e dev --features openapi scheduler
```

## Note sur la production

Les endpoints `/__r2e_dev/*` ne doivent **pas** etre actives en production. Ne pas appeler `.with_dev_reload()` dans le profil de production :

```rust
let mut builder = AppBuilder::new()
    .with_state(services)
    .with_health();

if is_dev {
    builder = builder.with_dev_reload();
}

builder.serve("0.0.0.0:3000").await.unwrap();
```

## Critere de validation

```bash
# Status
curl http://localhost:3000/__r2e_dev/status
# → dev

# Ping
curl http://localhost:3000/__r2e_dev/ping | jq .
# → {"boot_time": 1706123456789, "status": "ok"}

# Apres un redemarrage, boot_time change
```
