# Beans & Dependency Injection (r2e-core, r2e-macros)

## Three bean traits

| Trait | Constructor | Registration | Use case |
|-------|-----------|-------------|----------|
| `Bean` | `fn build(ctx) -> Self` (sync) | `.with_bean::<T>()` | Simple services |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.with_async_bean::<T>()` | Services needing async init |
| `Producer` | `async fn produce(ctx) -> Output` | `.with_producer::<P>()` | Types you don't own (pools, clients) |

All three traits have an associated `type Deps` that declares their dependencies as a type-level list (e.g., `type Deps = TCons<LocalEventBus, TNil>`). This is generated automatically by the `#[bean]`, `#[derive(Bean)]`, and `#[producer]` macros. For manual impls without dependencies, use `type Deps = TNil;`.

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

When `#[config]` is used, `R2eConfig` is automatically added to the dependency list. Missing config keys panic with a message including the env var equivalent (e.g., `APP_DB_URL`).

## Key files

- `r2e-core/src/beans.rs` — `Bean`, `AsyncBean`, `Producer`, `BeanContext`, `BeanRegistry`
- `r2e-core/src/builder.rs` — `with_bean()`, `with_async_bean()`, `with_producer()`, async `build_state()`
- `r2e-macros/src/bean_attr.rs` — `#[bean]` (sync + async detection, `#[config]` param support, `#[consumer]` scanning + `EventSubscriber` generation)
- `r2e-macros/src/bean_derive.rs` — `#[derive(Bean)]` (`#[inject]` + `#[config]` field support)
- `r2e-macros/src/producer_attr.rs` — `#[producer]` macro
- `r2e-core/src/event_subscriber.rs` — `EventSubscriber` trait (for beans with `#[consumer]` methods)

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
