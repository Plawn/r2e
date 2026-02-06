# Feature 8 — Scheduling

## Objectif

Executer des taches de fond de maniere periodique (intervalle fixe ou expression cron), avec arret propre via `CancellationToken`.

## Concepts cles

### Scheduler

Le gestionnaire de taches planifiees. Il collecte les taches puis les demarre comme des taches Tokio en arriere-plan.

### ScheduledTask

Une tache individuelle avec un nom, un type de planification (`Schedule`), et une closure async qui recoit l'etat applicatif.

### Schedule

Enum definissant quand la tache s'execute :
- `Every(Duration)` — intervalle fixe
- `EveryDelay { interval, initial_delay }` — intervalle avec delai initial
- `Cron(String)` — expression cron

### CancellationToken

Token de `tokio-util` permettant d'arreter proprement les taches planifiees (typiquement a l'arret du serveur).

## Utilisation

### 1. Ajouter les dependances

```toml
[dependencies]
r2e-scheduler = { path = "../r2e-scheduler" }
tokio-util = { version = "0.7", features = ["rt"] }
```

### 2. Creer un Scheduler et ajouter des taches

```rust
use std::time::Duration;
use r2e_scheduler::{Scheduler, ScheduledTask, Schedule};
use tokio_util::sync::CancellationToken;

let cancel = CancellationToken::new();
let mut scheduler = Scheduler::new();

// Tache executee toutes les 30 secondes
scheduler.add_task(ScheduledTask {
    name: "user-count".to_string(),
    schedule: Schedule::Every(Duration::from_secs(30)),
    task: Box::new(|state: Services| {
        Box::pin(async move {
            let count = state.user_service.count().await;
            tracing::info!(count, "Nombre d'utilisateurs");
        })
    }),
});
```

### 3. Demarrer le scheduler

```rust
// Demarre toutes les taches en arriere-plan
scheduler.start(services.clone(), cancel.clone());
```

Les taches tournent jusqu'a ce que le `CancellationToken` soit annule.

### 4. Arreter le scheduler

```rust
// A l'arret de l'application
cancel.cancel();
```

Typiquement place apres `AppBuilder::serve()` :

```rust
AppBuilder::new()
    .with_state(services)
    // ...
    .serve("0.0.0.0:3000")
    .await
    .unwrap();

// Le serveur s'est arrete → arreter le scheduler
cancel.cancel();
```

## Types de planification

### Schedule::Every

Execute la tache a intervalle fixe, immediatement au demarrage puis a chaque tick :

```rust
Schedule::Every(Duration::from_secs(60))  // Toutes les 60 secondes
```

### Schedule::EveryDelay

Comme `Every`, mais avec un delai avant la premiere execution :

```rust
Schedule::EveryDelay {
    interval: Duration::from_secs(60),
    initial_delay: Duration::from_secs(10),  // Attendre 10s avant de commencer
}
```

### Schedule::Cron

Expression cron (6 champs : sec min hour day month weekday) :

```rust
Schedule::Cron("0 */5 * * * *".to_string())  // Toutes les 5 minutes
Schedule::Cron("0 0 * * * *".to_string())     // Toutes les heures
Schedule::Cron("0 0 2 * * *".to_string())     // Tous les jours a 2h
```

**Note** : le cron utilise la crate `cron` avec 6 champs (les secondes en premier).

## Acces a l'etat

Chaque tache recoit une copie de l'etat applicatif (`T: Clone`). Cela signifie que les taches ont acces aux services, au pool de base de donnees, au bus d'evenements, etc. :

```rust
scheduler.add_task(ScheduledTask {
    name: "cleanup-expired-sessions".to_string(),
    schedule: Schedule::Every(Duration::from_secs(300)),
    task: Box::new(|state: Services| {
        Box::pin(async move {
            sqlx::query("DELETE FROM sessions WHERE expired_at < datetime('now')")
                .execute(&state.pool)
                .await
                .ok();
        })
    }),
});
```

## Logs

Le scheduler log automatiquement le demarrage et l'arret de chaque tache :

```
INFO task="user-count" "Scheduled task started"
DEBUG task="user-count" "Executing scheduled task"
INFO task="user-count" "Scheduled task stopped"
```

## Critere de validation

Lancer l'application :

```bash
cargo run -p example-app
```

Dans les logs, toutes les 30 secondes :

```
INFO count=2 "Scheduled user count"
```

A l'arret (Ctrl+C), le scheduler s'arrete proprement via le `CancellationToken`.
