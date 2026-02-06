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

## Workflow hot-reload

Le mode dev est prevu pour fonctionner avec `cargo-watch` ou le CLI `r2e dev` :

```
1. Developpeur modifie un fichier .rs
2. cargo-watch detecte le changement
3. cargo-watch tue le serveur et le relance
4. Le nouveau processus a un nouveau boot_time
5. Un script/navigateur qui poll /__r2e_dev/ping detecte le changement
6. Le navigateur se rafraichit automatiquement
```

### Exemple de script de poll (JavaScript cote client)

```javascript
let lastBootTime = null;

setInterval(async () => {
    try {
        const resp = await fetch('/__r2e_dev/ping');
        const data = await resp.json();
        if (lastBootTime && data.boot_time !== lastBootTime) {
            window.location.reload();
        }
        lastBootTime = data.boot_time;
    } catch {
        // Serveur en cours de redemarrage
    }
}, 1000);
```

### Utilisation avec cargo-watch

```bash
cargo watch -x 'run -p example-app'
```

Ou avec le CLI R2E :

```bash
r2e dev
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
