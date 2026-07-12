# State and Beans

R2E's dependency injection is built on a bean graph — a set of factories that produce your application services in dependency order.

## Application state is inferred — you never write it

There is no hand-written state struct. You do not define a `Services`/`AppState`
type, and there is no `#[derive(BeanState)]`. Instead:

1. **Beans form the graph.** Each `.provide(value)` or `.register::<T>()` call adds
   a node. The builder threads the set of provided types through the chain at the
   type level (the *provision list*).
2. **State is the provision list materialized as an HList.** `build_state()`
   resolves the graph in dependency order and packs every provided value into a
   heterogeneous list whose shape is inferred from your `.provide` / `.register`
   calls. The state **type is inferred** — you never spell it out.
3. **You read beans out of the state by type**, not by field name (see
   [Reading beans from state](#reading-beans-from-state)).

```rust
use r2e::prelude::*;

let app = AppBuilder::new()
    .provide(event_bus)            // LocalEventBus becomes a bean
    .provide(pool)                 // SqlitePool becomes a bean
    .register::<UserService>();    // a #[bean] type, constructed from the graph

app.build_state()                  // NO type args; async — resolves the graph
    .await
    // ... plugins, controllers, serve ...
    ;
```

> Apps with more than ~127 provisions need `#![recursion_limit = "512"]` at the
> crate root (in `main.rs`) — the inferred HList is a deeply nested type and the
> default recursion limit (128) is not enough. `r2e doctor` warns as the bean
> count approaches the threshold. Fewer than ~127 beans needs nothing.

## Bean traits

R2E provides three bean traits for registering services:

### `Bean` — Synchronous construction

For services with simple, synchronous initialization:

```rust
#[derive(Clone)]
pub struct UserService {
    pool: SqlitePool,
    event_bus: LocalEventBus,
}

#[bean]
impl UserService {
    pub fn new(pool: SqlitePool, event_bus: LocalEventBus) -> Self {
        Self { pool, event_bus }
    }
}
```

Register with `.register::<UserService>()`.

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

Register with `.register::<CacheService>()`. The `#[bean]` macro auto-detects async constructors and generates `impl AsyncBean` instead of `impl Bean`.

### `Producer` — Factory for types you don't own

For types from external crates (e.g., connection pools) where you can't write `impl Bean`:

```rust
#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
```

This generates a struct `CreatePool` (PascalCase of the function name) with `impl Producer`. Register with `.register::<CreatePool>()`. The struct is just a vehicle for the trait impl — you never instantiate it yourself.

Producer parameters can be **bean dependencies**, **config values**, or both:

```rust
#[producer]
async fn create_notifier(
    bus: LocalEventBus,                                // ← resolved from BeanContext
    #[config("notification.url")] url: String,    // ← resolved from R2eConfig
    #[config("notification.timeout")] timeout: i64,
) -> NotificationClient {
    NotificationClient::new(&url, timeout, bus).await
}
// Generates: CreateNotifier with deps [LocalEventBus, R2eConfig]
// Register: .register::<CreateNotifier>()
```

Parameters without `#[config]` are treated as bean dependencies (pulled from `ctx.get::<T>()`). Parameters with `#[config("key")]` are resolved from `R2eConfig` — and `R2eConfig` is automatically added to the dependency list when any `#[config]` param is present.

### Conditional availability with `Option<T>`

A producer may return `Option<T>` to express that a service is only available under certain conditions (e.g., a feature flag, an optional API key):

```rust
#[producer]
async fn create_llm_client(
    #[config("app.llm.api_key")] api_key: Option<String>,
) -> Option<Arc<LlmClient>> {
    let key = api_key?;
    Some(Arc::new(LlmClient::new(&key)))
}
```

`Option<T>` is a **first-class bean type** in R2E: the slot is registered under `TypeId::of::<Option<T>>()` — separate from `TypeId::of::<T>()` — and is **always** present in the graph. The value (`Some`/`None`) reflects the producer's decision.

Consumers then **hard-depend** on `Option<T>` and decide how to behave based on the inner `Option`:

```rust
#[bean]
impl ChatService {
    fn new(llm: Option<Arc<LlmClient>>) -> Self {
        Self { llm }
    }
}
```

This gives you a single, honest knob: the producer encapsulates the decision, the consumer handles both branches, and the compile-time dependency graph catches missing registrations. This is the blessed pattern for conditional beans consumed by macro-derived beans — the `Option<T>` slot is always registered, so the graph stays complete regardless of the runtime decision. (For conditional plugins or layers that don't change the provision list, use `.when(cond, |b| ...)` with the `config_flag(key)` / `profile_is(profile)` helpers.)

### `#[derive(Bean)]` — Derive-based beans

For simple structs where the constructor just clones fields from the graph:

```rust
#[derive(Clone, Bean)]
pub struct MyService {
    #[inject] event_bus: LocalEventBus,
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
        bus: LocalEventBus,
        #[config("notification.capacity")] capacity: i64,
        #[config("notification.enabled")] enabled: bool,
    ) -> Self {
        Self { bus, capacity: capacity as usize, enabled }
    }
}
```

When `#[config]` is used, `R2eConfig` is automatically added to the bean's dependency list. Missing keys panic with an error that includes the environment variable equivalent.

## Post-construct hooks

Sometimes a bean needs to perform initialization *after* the entire bean graph is resolved — for example, warming a cache, running migrations, or cleaning up stale data. Use `#[post_construct]` for this:

```rust
#[derive(Clone)]
pub struct CacheService {
    pool: SqlitePool,
}

#[bean]
impl CacheService {
    pub async fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[post_construct]
    async fn warm_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // pre-load frequently accessed data
        let _rows = sqlx::query("SELECT * FROM hot_data")
            .fetch_all(&*self.pool)
            .await?;
        Ok(())
    }
}
```

Key points:
- Methods must take `&self` only — no additional parameters
- Return `()` or `Result<(), Box<dyn Error + Send + Sync>>`
- Can be `async`
- Multiple `#[post_construct]` methods run in declaration order
- Hooks run after **all** beans are constructed, in dependency order
- If any hook returns an error, `build_state()` fails with `BeanError::PostConstruct`

**Do** use `#[post_construct]` for: cache warming, stale data cleanup, migrations, cross-bean validation.

**Don't** use it for: construction logic (belongs in the constructor), event subscriptions (`#[consumer]`), periodic work (`#[scheduled]`).

### Lifecycle for pre-built (`provide`-d) beans

`#[post_construct]` attaches to factory beans (`#[bean]` / `register`). A value
handed to `.provide(instance)` can opt into the **same** lifecycle explicitly —
and into disposal via the symmetric `PreDestroy` trait:

```rust,ignore
AppBuilder::new()
    // T: PostConstruct — hook runs during build_state(), after every
    // factory-bean post-construct.
    .provide_with_post_construct(cache)
    // T: PreDestroy — pre_destroy() runs during graceful shutdown.
    .provide_with_pre_destroy(pool)
    .build_state().await;
```

Both hooks read the bean from the resolved graph by type (a pinned test
override is honoured); a failing post-construct fails `build_state()` with
`BeanError::PostConstruct`; disposers run in reverse registration order during
shutdown. Plugins opt their `Provided` beans in the same way via
`ctx.run_post_construct::<T>()` / `ctx.run_pre_destroy::<T>()`.

## Reading beans from state

Because the state is an inferred HList (not a struct with named fields), you read
beans out of it **by type**. Controller `#[inject]` fields resolve this way
automatically at registration time. Code that holds the state directly — guards,
interceptors, `ManagedResource`, plugins — uses one of two accessors:

- `state.bean::<T>() -> Option<T>` — dynamic lookup via the `BeanLookup` trait.
  In the prelude. This is the vocabulary for guards / interceptors /
  `ManagedResource`, which must be **generic over the state** (`S: BeanLookup`).
- `state.get::<T>() -> T` — witness-free fixed-offset access via `BeanAccess`.
  **Not** in the prelude (its blanket `get` would shadow `Deref`-reached inherent
  `get`s). Import it explicitly: `use r2e_core::type_list::BeanAccess;`.

```rust
use r2e_core::type_list::BeanAccess;

// generic over the state, reads a bean dynamically:
let pool = state.bean::<SqlitePool>().expect("SqlitePool bean");

// fixed-offset access when you hold a concrete state:
let pool: SqlitePool = state.get::<SqlitePool>();
```

A type that is not in the graph is a **compile error naming the type** for
`#[inject]` fields (checked via the controller's `Deps` against the provision
list), and `None`/an unsatisfied bound for the accessors.

## Building state

The `build_state()` method resolves the bean graph in dependency order and packs
the resolved values into the inferred HList state. It takes **no type arguments**
and is async (the graph may contain async beans):

```rust
AppBuilder::new()
    .provide(event_bus)                    // provide pre-built instances
    .provide(pool)
    .register::<CreatePool>()              // async producer
    .register::<CacheService>()            // async bean
    .register::<UserService>()             // sync bean
    // config sections are auto-registered by load_config (available as bean deps)
    .build_state()                         // resolve the graph — no type args
    .await                                 // async because graph may contain async beans
```

`try_build_state()` is the non-panicking (`Result`) variant.

### `provide()` vs `register()`

- `provide(value)` — injects a pre-built instance directly into the graph
- `register::<T>()` — registers a factory (bean, async bean, or producer); R2E constructs it from its dependencies

Use `provide()` for values constructed outside the bean graph (e.g., configuration, tokens, pre-existing pools).

### Resolution order

1. All `provide()`d values are available immediately
2. `Bean` / `AsyncBean` / `Producer` factories run in dependency order
3. If bean A depends on bean B, B is constructed first
4. Circular dependencies fail at startup with a concrete `A -> B -> A` path

## Complete example

```rust
#![recursion_limit = "512"] // only needed past ~127 beans; harmless otherwise

use r2e::prelude::*;

#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}

#[tokio::main]
async fn main() {
    let event_bus = LocalEventBus::new();

    AppBuilder::new()
        .load_config::<()>()
        .provide(event_bus)
        .register::<CreatePool>()
        .register::<UserService>()
        .register::<NotificationService>()
        .build_state()                  // state type is inferred from the provisions
        .await
        // ... register controllers, plugins, etc.
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```
