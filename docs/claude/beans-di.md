# Beans & Dependency Injection (r2e-core, r2e-macros)

## Three bean traits

| Trait | Constructor | Registration | Use case |
|-------|-----------|-------------|----------|
| `Bean` | `fn build(ctx) -> Self` (sync) | `.with_bean::<T>()` | Simple services |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.with_async_bean::<T>()` | Services needing async init |
| `Producer` | `async fn produce(ctx) -> Output` | `.with_producer::<P>()` | Types you don't own (pools, clients) |

All three traits have an associated `type Deps` that declares their dependencies as a type-level list. **This is auto-generated** by the `#[bean]`, `#[derive(Bean)]`, and `#[producer]` macros — you never write `Deps` manually. For manual trait impls without dependencies, use `type Deps = TNil;`.

**`build_state()` is async** — it must be `.await`ed because the bean graph may contain async beans or producers. It takes 3 generic args: `build_state::<S, _, _>()` (state type, provisions, requirements).

## `#[bean]` attribute macro

Auto-detects sync vs async constructors:

```rust
// Sync → generates `impl Bean`
#[bean]
impl UserService {
    fn new(event_bus: LocalEventBus) -> Self { Self { event_bus } }
}

// Async → generates `impl AsyncBean`
#[bean]
impl MyAsyncService {
    async fn new(pool: SqlitePool) -> Self { /* ... */ Self { pool } }
}
```

## `#[producer]` attribute macro

For free functions producing types you don't own:

```rust
#[producer]
async fn create_pool(#[config("app.db.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
// Generates: struct CreatePool; impl Producer for CreatePool { type Output = SqlitePool; ... }
```

## Auto-registered config beans

When using `load_config::<RootConfig>()`, all `#[config(section)]` children in the root config are auto-registered as beans via `register_children`. This means beans can depend on nested config types directly:

```rust
#[bean]
impl SearchService {
    fn new(matching: MatchingConfig) -> Self {  // MatchingConfig from BeanContext (auto-registered)
        Self { matching }
    }
}
```

No manual `.provide()` or `#[config_section]` needed — `load_config` handles it.

## Optional dependencies (`Option<T>`)

A bean can declare a dependency as optional by wrapping it in `Option<T>`. Optional dependencies resolve to `None` when the inner type is not in the bean graph, and `Some(T)` when it is.

**Compile-time rules:**

| Dependency type | In `Deps` (type list) | In `dependencies()` | Resolution |
|---|---|---|---|
| `T` | Yes | Yes | `ctx.get::<T>()` — panics if absent |
| `Option<T>` | **No** | **No** | `ctx.try_get::<T>()` — `None` if absent |

Optional deps are invisible to the compile-time graph. No `Contains<T, _>` bound is generated. Hard deps (`T`) remain fully checked.

### In `#[bean]` constructor params

```rust
#[bean]
impl NotificationService {
    fn new(mailer: Mailer, cache: Option<RedisClient>) -> Self {
        Self { mailer, cache }
    }
}
```

### In `#[derive(Bean)]` fields

```rust
#[derive(Clone, Bean)]
struct MyService {
    #[inject] mailer: Mailer,
    #[inject] cache: Option<RedisClient>,
}
```

### In `#[producer]` params

```rust
#[producer]
async fn create_pool(metrics: Option<MetricsCollector>) -> SqlitePool {
    // metrics is None if MetricsCollector was not registered
    SqlitePool::connect("sqlite::memory:").await.unwrap()
}
```

### In `BeanState`

```rust
#[derive(Clone, BeanState)]
struct AppState {
    user_service: UserService,
    cache: Option<RedisClient>,  // no compile error if RedisClient not in P
}
```

`Option<T>` fields in `BeanState` do NOT generate a `BuildableFrom` bound for `T`.

### Composing with conditional registration

Optional dependencies compose naturally with conditional builder methods (see [subsystems.md](./subsystems.md)). Conditional beans are NOT in `P` (the provision list), so consumers MUST use `Option<T>` — the compiler enforces this.

```rust
AppBuilder::new()
    .with_bean::<UserService>()                    // always in P — guaranteed
    .with_bean_when::<RedisCache>(use_redis)       // NOT in P — consumers must use Option<RedisCache>
    .build_state::<AppState, _, _>().await
```

## `#[config("key")]` in beans

Resolve values from `R2eConfig` instead of the bean graph:

```rust
// In #[bean] constructor params:
#[bean]
impl NotificationService {
    fn new(bus: LocalEventBus, #[config("notification.capacity")] capacity: i64) -> Self { ... }
}

// In #[derive(Bean)] fields:
#[derive(Clone, Bean)]
struct MyService {
    #[inject] event_bus: LocalEventBus,
    #[config("app.name")] name: String,
}
```

When `#[config]` is used, `R2eConfig` (see [configuration.md](./configuration.md)) is automatically added to the dependency list. Missing config keys panic with a message including the env var equivalent (e.g., `APP_DB_URL`).

## `#[consumer]` on beans

Beans can declare event consumers using the same `#[consumer(bus = "field")]` syntax as controllers:

```rust
#[derive(Clone)]
pub struct NotificationService {
    event_bus: LocalEventBus,
    mailer: Mailer,
}

#[bean]
impl NotificationService {
    pub fn new(event_bus: LocalEventBus, mailer: Mailer) -> Self {
        Self { event_bus, mailer }
    }

    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        self.mailer.send_welcome(&event.email).await;
    }
}
```

When `#[consumer]` methods are present, the `#[bean]` macro generates an `EventSubscriber` impl. Register via `register_subscriber::<NotificationService>()` on the builder.

Multiple buses of different types are supported — each `#[consumer]` references a different field by name.

## `#[post_construct]`

Lifecycle hooks called **after the entire bean graph is resolved**. All dependencies are available when hooks fire.

### Constraints

- Method signature: `fn name(&self)` or `async fn name(&self)` — no extra parameters.
- Return type: `()` or `Result<(), Box<dyn Error + Send + Sync>>`.
- Multiple `#[post_construct]` methods on a single bean are called in **declaration order**.
- Execution order across beans: **topological order** (same as construction order).
- If a hook returns `Err`, `build_state()` returns `BeanError::PostConstruct(String)`.

### Example

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
        // load frequently accessed data into memory
        Ok(())
    }

    #[post_construct]
    fn log_ready(&self) {
        tracing::info!("CacheService ready");
    }
}
```

### Generated code

The `#[bean]` macro generates:
1. `impl PostConstruct for CacheService` — wraps all `#[post_construct]` methods into a single async future, calling them in declaration order.
2. `fn after_register(registry)` on the `Bean`/`AsyncBean` impl — calls `registry.register_post_construct::<Self>()`.

### When to use

| Use `#[post_construct]` for | Don't use for |
|---|---|
| Cache warming | Construction logic (use the constructor) |
| Stale data cleanup | Registering event listeners (use `#[consumer]`) |
| Database migrations | Periodic tasks (use `#[scheduled]`) |
| Validation that needs other beans | Simple field init |

## Key files

- `r2e-core/src/beans.rs` — `Bean`, `AsyncBean`, `Producer`, `PostConstruct`, `BeanContext`, `BeanRegistry`
- `r2e-core/src/builder.rs` — `with_bean()`, `with_async_bean()`, `with_producer()`, async `build_state()`
- `r2e-macros/src/bean_attr.rs` — `#[bean]` (sync + async detection, `#[config]` param support, `Option<T>` detection, `#[consumer]` scanning + `EventSubscriber` generation, `scan_post_construct_methods` + `PostConstruct` generation)
- `r2e-macros/src/bean_derive.rs` — `#[derive(Bean)]` (`#[inject]` + `#[config]` field support, `Option<T>` detection)
- `r2e-macros/src/bean_state_derive.rs` — `#[derive(BeanState)]` (`Option<T>` field support — `try_get` + skips `BuildableFrom` bounds)
- `r2e-macros/src/producer_attr.rs` — `#[producer]` macro (`Option<T>` detection)
- `r2e-macros/src/type_utils.rs` — `unwrap_option_type()` helper shared by all bean macros
- `r2e-core/src/event_subscriber.rs` — `EventSubscriber` trait (for beans with `#[consumer]` methods)
