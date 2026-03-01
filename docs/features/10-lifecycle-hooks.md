# Feature 10 — Hooks de cycle de vie

## Objectif

Permettre d'executer du code au demarrage et a l'arret du serveur, pour initialiser des ressources ou effectuer du nettoyage.

## Concepts cles

### on_start

Hook execute **avant** que le serveur commence a ecouter les connexions. Recoit l'etat applicatif en parametre. Peut retourner une erreur pour empecher le demarrage.

### on_stop

Hook execute **apres** l'arret gracieux du serveur (signal Ctrl+C ou SIGTERM). Ne recoit pas l'etat, ne peut pas echouer.

## Utilisation

### 1. Hook de demarrage

```rust
AppBuilder::new()
    .with_state(services)
    .on_start(|state| async move {
        // Verifier la connexion a la base de donnees
        sqlx::query("SELECT 1").execute(&state.pool).await?;
        tracing::info!("Connexion DB verifiee");

        // Initialiser des donnees
        tracing::info!("Application demarree");
        Ok(())
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Signature du hook de demarrage

```rust
FnOnce(T) -> Future<Output = Result<(), Box<dyn Error + Send + Sync>>>
```

- Recoit `T` (l'etat applicatif, clone)
- Doit retourner `Ok(())` pour permettre le demarrage
- Si retourne `Err(...)`, le serveur ne demarre pas et l'erreur est propagee

### Exemple : seed d'un utilisateur admin depuis les variables d'environnement

Un pattern courant consiste a creer un utilisateur admin initial au demarrage. `on_start` donne acces a l'etat complet (services DI resolus), et `R2eConfig` overlay automatiquement les variables d'environnement (`ADMIN_EMAIL` → `admin.email`) — pas besoin de `std::env::var()` ni de reconstruire les repositories manuellement :

```rust
AppBuilder::new()
    .with_state(services)
    .on_start(|state| async move {
        // R2eConfig mappe ADMIN_EMAIL → admin.email automatiquement
        let email: String = state.config.get("admin.email").unwrap_or_default();
        let password: String = state.config.get("admin.password").unwrap_or_default();

        if email.is_empty() || password.is_empty() {
            return Ok(()); // pas de seed demande
        }

        // user_repo est deja dans le state via DI
        if state.user_repo.find_by_email(&email).await?.is_some() {
            tracing::debug!("Admin seed skipped — {} already exists", email);
            return Ok(());
        }

        let hash = hash_password(&password)?;
        state.user_repo.create(&NewUser {
            email: email.clone(),
            role: Role::Admin,
            password_hash: Some(hash),
            ..Default::default()
        }).await?;

        tracing::info!("Admin user seeded: {}", email);
        Ok(())
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

Points cles :
- **Pas de `std::env::var()`** — utiliser `state.config.get()`. Les variables d'environnement sont automatiquement mappees (`ADMIN_EMAIL` → `admin.email`).
- **Pas de construction manuelle des repositories** — les services sont deja disponibles dans le state via DI.
- **Utiliser `?` pour la propagation d'erreurs** — `on_start` retourne `Result`, donc les erreurs bloquent proprement le demarrage au lieu d'etre silencieusement loguees.

### 2. Hook d'arret

```rust
AppBuilder::new()
    .with_state(services)
    .on_stop(|| async {
        tracing::info!("Arret en cours...");
        // Nettoyage, flush des logs, fermeture de connexions...
        tracing::info!("Nettoyage termine");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Signature du hook d'arret

```rust
FnOnce() -> Future<Output = ()>
```

- Ne recoit pas l'etat (le serveur est deja arrete)
- Ne peut pas echouer (retourne `()`)

### 3. Plusieurs hooks

Les deux methodes peuvent etre appelees plusieurs fois. Les hooks sont executes dans l'ordre d'enregistrement :

```rust
AppBuilder::new()
    .with_state(services)
    .on_start(|state| async move {
        tracing::info!("Hook 1 : verification DB");
        sqlx::query("SELECT 1").execute(&state.pool).await?;
        Ok(())
    })
    .on_start(|_state| async move {
        tracing::info!("Hook 2 : chargement cache");
        Ok(())
    })
    .on_stop(|| async {
        tracing::info!("Hook arret 1");
    })
    .on_stop(|| async {
        tracing::info!("Hook arret 2");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Grace period de shutdown

Par defaut, R2E attend indefiniment que les hooks de shutdown terminent. Utilisez `shutdown_grace_period` pour definir un delai maximum — si les hooks ne finissent pas dans le temps imparti, le processus force l'arret :

```rust
use std::time::Duration;

AppBuilder::new()
    .with_state(services)
    .shutdown_grace_period(Duration::from_secs(5))
    .on_stop(|| async {
        tracing::info!("Nettoyage...");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

Cela remplace le pattern courant ou les utilisateurs spawnent manuellement un handler de shutdown avec `CancellationToken` + `tokio::signal::ctrl_c()` + `process::exit()`.

## Ordre d'execution

```
1. on_start hooks (sequentiels, dans l'ordre d'enregistrement)
2. Serveur commence a ecouter (bind TCP)
3. ... traitement des requetes ...
4. Signal d'arret recu (Ctrl+C / SIGTERM)
5. Arret gracieux du serveur
6. on_stop hooks (sequentiels, dans l'ordre d'enregistrement)
7. Si grace period definie et depassee → force exit
```

### Echec d'un hook de demarrage

Si un `on_start` retourne `Err`, l'execution s'arrete immediatement :
- Les hooks suivants ne sont **pas** executes
- Le serveur ne commence **pas** a ecouter
- L'erreur est propagee a l'appelant de `serve()`

## Cas d'usage typiques

### Demarrage

| Usage | Exemple |
|-------|---------|
| Verification de connectivite | Tester la connexion DB avant d'accepter des requetes |
| Migration de schema | Executer des migrations au demarrage |
| Seed de donnees | Creer un admin initial depuis les variables d'environnement |
| Chargement de cache | Pre-remplir un cache en memoire |
| Verification de configuration | Valider que toutes les cles requises sont presentes |
| Log informatif | Afficher la version, le profil actif, etc. |

### Arret

| Usage | Exemple |
|-------|---------|
| Flush de logs/metriques | Envoyer les metriques restantes avant l'arret |
| Fermeture de connexions | Fermer proprement les connexions externes |
| Notification | Prevenir un systeme de monitoring de l'arret |
| Sauvegarde d'etat | Persister un etat en memoire sur disque |

## Trait LifecycleController

Pour les cas plus avances, le trait `LifecycleController` permet de definir les hooks directement sur un controller :

```rust
impl LifecycleController<Services> for MyController {
    fn on_start(state: &Services) -> Pin<Box<dyn Future<Output = Result<...>> + Send + '_>> {
        Box::pin(async move {
            tracing::info!("MyController starting");
            Ok(())
        })
    }

    fn on_stop() -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {
            tracing::info!("MyController stopping");
        })
    }
}
```

## Critere de validation

```bash
cargo run -p example-app
```

Au demarrage :

```
INFO "R2E example-app startup hook executed"
INFO addr="0.0.0.0:3000" "R2E server listening"
```

A l'arret (Ctrl+C) :

```
INFO "Shutdown signal received, starting graceful shutdown"
INFO "R2E example-app shutdown hook executed"
INFO "R2E server stopped"
```
