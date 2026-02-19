# State and Beans

R2E's dependency injection is built on a bean graph — a set of factories that produce your application services in dependency order.

## Application state

Your state struct holds all app-scoped dependencies. Derive `BeanState` to generate `FromRef` implementations:

```rust
use r2e::prelude::*;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub user_service: UserService,
    pub pool: SqlitePool,
    pub event_bus: EventBus,
    pub config: R2eConfig,
}
```

## Bean traits

R2E provides three bean traits for registering services:

### `Bean` — Synchronous construction

For services with simple, synchronous initialization:

```rust
#[derive(Clone)]
pub struct UserService {
    pool: SqlitePool,
    event_bus: EventBus,
}

#[bean]
impl UserService {
    pub fn new(pool: SqlitePool, event_bus: EventBus) -> Self {
        Self { pool, event_bus }
    }
}
```

Register with `.with_bean::<UserService>()`.

### `AsyncBean` — Asynchronous construction

For services that need async initialization (e.g., database connections):

```rust
#[derive(Clone)]
pub struct CacheService {
    client: RedisClient,
}

#[bean]
impl CacheService {
    pub async fn new(#[config("cache.url")] url: String) -> Self {
        let client = RedisClient::connect(&url).await.unwrap();
        Self { client }
    }
}
```

Register with `.with_async_bean::<CacheService>()`. The `#[bean]` macro auto-detects async constructors and generates `impl AsyncBean` instead of `impl Bean`.

### `Producer` — Factory for types you don't own

For types from external crates (e.g., connection pools) where you can't write `impl Bean`:

```rust
#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
```

This generates a struct `CreatePool` (PascalCase of the function name) with `impl Producer`. Register with `.with_producer::<CreatePool>()`. The struct is just a vehicle for the trait impl — you never instantiate it yourself.

Producer parameters can be **bean dependencies**, **config values**, or both:

```rust
#[producer]
async fn create_notifier(
    bus: EventBus,                                // ← resolved from BeanContext
    #[config("notification.url")] url: String,    // ← resolved from R2eConfig
    #[config("notification.timeout")] timeout: i64,
) -> NotificationClient {
    NotificationClient::new(&url, timeout, bus).await
}
// Generates: CreateNotifier with deps [EventBus, R2eConfig]
// Register: .with_producer::<CreateNotifier>()
```

Parameters without `#[config]` are treated as bean dependencies (pulled from `ctx.get::<T>()`). Parameters with `#[config("key")]` are resolved from `R2eConfig` — and `R2eConfig` is automatically added to the dependency list when any `#[config]` param is present.

### `#[derive(Bean)]` — Derive-based beans

For simple structs where the constructor just clones fields from the graph:

```rust
#[derive(Clone, Bean)]
pub struct MyService {
    #[inject] event_bus: EventBus,
    #[inject] pool: SqlitePool,
    #[config("app.name")] name: String,
}
```

## Config injection in beans

Use `#[config("key")]` on constructor parameters or derive fields to resolve values from `R2eConfig`:

```rust
#[bean]
impl NotificationService {
    pub fn new(
        bus: EventBus,
        #[config("notification.capacity")] capacity: i64,
        #[config("notification.enabled")] enabled: bool,
    ) -> Self {
        Self { bus, capacity: capacity as usize, enabled }
    }
}
```

When `#[config]` is used, `R2eConfig` is automatically added to the bean's dependency list. Missing keys panic with an error that includes the environment variable equivalent.

## Building state

The `build_state()` method resolves the bean graph in dependency order:

```rust
AppBuilder::new()
    .provide(event_bus)                    // provide pre-built instances
    .provide(pool)
    .with_producer::<CreatePool>()         // async producer
    .with_async_bean::<CacheService>()     // async bean
    .with_bean::<UserService>()            // sync bean
    .build_state::<AppState, _>()          // resolve the graph
    .await                                 // async because graph may contain async beans
```

### `provide()` vs `with_bean()`

- `provide(value)` — injects a pre-built instance directly into the graph
- `with_bean::<T>()` — registers a factory; R2E constructs it from its dependencies

Use `provide()` for values constructed outside the bean graph (e.g., configuration, tokens, pre-existing pools).

### Resolution order

1. All `provide()`d values are available immediately
2. `Bean` / `AsyncBean` / `Producer` factories run in dependency order
3. If bean A depends on bean B, B is constructed first
4. Circular dependencies cause a panic at startup

## Complete example

```rust
use r2e::prelude::*;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub user_service: UserService,
    pub notification_service: NotificationService,
    pub pool: SqlitePool,
    pub event_bus: EventBus,
    pub config: R2eConfig,
}

#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}

#[tokio::main]
async fn main() {
    let config = R2eConfig::load("dev").unwrap();
    let event_bus = EventBus::new();

    AppBuilder::new()
        .provide(event_bus)
        .provide(config.clone())
        .with_producer::<CreatePool>()
        .with_bean::<UserService>()
        .with_bean::<NotificationService>()
        .build_state::<AppState, _>()
        .await
        .with_config(config)
        // ... register controllers, plugins, etc.
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```
