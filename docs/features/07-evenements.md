# Feature 7 — Evenements

## Objectif

Fournir un bus d'evenements in-process avec pub/sub type. Permet de decoupler les composants de l'application en emettant des evenements que d'autres parties peuvent ecouter.

## Concepts cles

### EventBus

Le bus d'evenements central. Il est `Clone` et peut etre partage entre threads. Le dispatch est base sur le `TypeId` — chaque type d'evenement a ses propres abonnes.

### Typage fort

Les evenements sont dispatches par type Rust. Un abonne a `UserCreatedEvent` ne recevra jamais un `OrderPlacedEvent`. Pas de strings magiques, pas de downcasting manuel.

## Utilisation

### 1. Ajouter la dependance

```toml
[dependencies]
quarlus-events = { path = "../quarlus-events" }
```

### 2. Definir un type d'evenement

```rust
#[derive(Debug, Clone)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}
```

Le type doit etre `Send + Sync + 'static`. Pas besoin d'implementer de trait special.

### 3. Creer le bus et s'abonner

```rust
use std::sync::Arc;
use quarlus_events::EventBus;

let event_bus = EventBus::new();

// S'abonner a un type d'evenement
event_bus
    .subscribe(|event: Arc<UserCreatedEvent>| async move {
        tracing::info!(
            user_id = event.user_id,
            name = %event.name,
            email = %event.email,
            "Nouvel utilisateur cree"
        );
    })
    .await;
```

**Note** : le handler recoit `Arc<E>` (pas `E` directement), car l'evenement peut etre partage entre plusieurs abonnes.

### Abonnes multiples

Plusieurs handlers peuvent ecouter le meme type :

```rust
// Handler 1 : log
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    tracing::info!("User created: {}", event.name);
}).await;

// Handler 2 : notification email
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    send_welcome_email(&event.email).await;
}).await;

// Handler 3 : analytics
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    track_signup(event.user_id).await;
}).await;
```

### 4. Emettre un evenement

```rust
// Emission fire-and-forget (les handlers tournent en taches Tokio paralleles)
event_bus.emit(UserCreatedEvent {
    user_id: 42,
    name: "Alice".into(),
    email: "alice@example.com".into(),
}).await;

// Emission avec attente de completion de tous les handlers
event_bus.emit_and_wait(UserCreatedEvent {
    user_id: 42,
    name: "Alice".into(),
    email: "alice@example.com".into(),
}).await;
```

### Difference `emit` vs `emit_and_wait`

| Methode | Comportement |
|---------|-------------|
| `emit()` | Spawn les handlers en taches Tokio, retourne immediatement |
| `emit_and_wait()` | Spawn les handlers, attend que tous se terminent |

### 5. Integration dans un service

Typiquement, le `EventBus` est injecte dans les services :

```rust
#[derive(Clone)]
pub struct UserService {
    users: Arc<RwLock<Vec<User>>>,
    event_bus: EventBus,
}

impl UserService {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            users: Arc::new(RwLock::new(vec![/* ... */])),
            event_bus,
        }
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let user = {
            let mut users = self.users.write().await;
            let id = users.len() as u64 + 1;
            let user = User { id, name, email };
            users.push(user.clone());
            user
        }; // Lock relache ici

        // Emettre l'evenement apres le lock
        self.event_bus
            .emit(UserCreatedEvent {
                user_id: user.id,
                name: user.name.clone(),
                email: user.email.clone(),
            })
            .await;

        user
    }
}
```

### 6. Partager le bus via l'etat applicatif

```rust
#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,
    pub event_bus: EventBus,
    // ...
}

impl axum::extract::FromRef<Services> for EventBus {
    fn from_ref(state: &Services) -> Self {
        state.event_bus.clone()
    }
}
```

## Isolation par type

Les evenements sont completement isoles par `TypeId`. Emettre un `OtherEvent` ne declenche pas les handlers de `UserCreatedEvent` :

```rust
struct OtherEvent;

bus.subscribe(|_: Arc<UserCreatedEvent>| async { println!("user!"); }).await;
bus.emit(OtherEvent).await;
// → rien ne se passe, le handler de UserCreatedEvent n'est pas appele
```

## Critere de validation

Lancer l'application et creer un utilisateur :

```bash
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@example.com"}'
```

Dans les logs du serveur :

```
INFO user_id=3 name="Alice" email="alice@example.com" "User created event received"
```
