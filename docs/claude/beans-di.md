# Beans & Dependency Injection (r2e-core, r2e-macros)

## Three bean traits

| Trait | Constructor | Registration | Use case |
|-------|-----------|-------------|----------|
| `Bean` | `fn build(ctx) -> Self` (sync) | `.register::<T>()` | Simple services |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.register::<T>()` | Services needing async init |
| `Producer` | `async fn produce(ctx) -> Output` | `.register::<P>()` | Types you don't own (pools, clients) |

All three kinds register through the single unified `.register::<T>()` method — the type implements `Registrable`, which `#[bean]`, `#[derive(Bean)]`, and `#[producer]` emit automatically. `#[bean]` picks sync vs async `Bean`/`AsyncBean` for you; `#[producer]` registers the producer's **output** type.

All three traits have an associated `type Deps` that declares their dependencies as a type-level list. **This is auto-generated** by the `#[bean]`, `#[derive(Bean)]`, and `#[producer]` macros — you never write `Deps` manually. For manual trait impls without dependencies, use `type Deps = TNil;`.

**`build_state` is async and takes NO type arguments** — it must be `.await`ed because the bean graph may contain async beans or producers. The state type is **inferred**: it is the builder's provision list `P` (everything you `.provide()`/`.register()`) materialized as a type-level HList. You never write a state struct. Call `.build_state().await` directly; `.try_build_state().await` is the non-panicking (`Result`) variant.

```rust
let app = AppBuilder::new()
    .load_config::<RootConfig>()
    .provide(event_bus)
    .provide(pool)
    .register::<UserService>();

let built = app.build_state().await;   // no turbofish, no state struct
```

**recursion_limit** — the HList access machinery recurses once per provision. Apps with more than ~127 provisions need `#![recursion_limit = "512"]` at the crate root (top of `main.rs`); below that the default limit is fine. `r2e doctor` warns as the bean count approaches the threshold.

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

### Conditional availability via `Option<T>` (first-class bean type)

`Option<T>` is a distinct bean type: a producer that declares
`type Output = Option<T>` (and the `#[producer]` macro infers this from the
return type) registers `Option<T>` in the context, **always** — the slot is
guaranteed to exist. The value is whatever the producer body returns:
`Some(...)` or `None`.

Consumers inject `Option<T>` as a **hard** dependency (not a soft / fallible
lookup) — the graph guarantees the slot is present, and the consumer decides
how to behave based on the inner `Option`:

```rust
#[producer]
async fn create_llm_client(
    #[config("app.llm.api_key")] api_key: Option<String>,
) -> Option<Arc<LlmClient>> {
    let key = api_key?; // producer returns None → slot is Some/None
    Some(Arc::new(LlmClient::new(&key)))
}

#[bean]
impl ChatService {
    fn new(llm: Option<Arc<LlmClient>>) -> Self { Self { llm } }
    //              ^^^^^^^^^^^^^^^^^ hard dep on Option<Arc<LlmClient>>
}
```

Internally:

- `Producer::Output = Option<Arc<LlmClient>>` — registered under
  `TypeId::of::<Option<Arc<LlmClient>>>()`
- `ChatService::dependencies()` lists `Option<Arc<LlmClient>>` as a hard dep
- The topological sort orders `ChatService` after `CreateLlmClient`
- If you want a hard dep on the inner type instead (panic when absent),
  declare the param as `llm: Arc<LlmClient>` — but this still requires a
  producer that returns `Arc<LlmClient>` directly (not `Option<...>`).

**Conditional bean presence:** keep the slot in the provision list and let
the producer decide. A `#[producer] -> Option<T>` always registers the
`Option<T>` slot, so macro-derived consumers that declare `Option<T>` as a
dep always compile; the value reflects config/runtime state. This is the
blessed path — there is no `_when`-style builder method that skips
registration (those would leave the slot missing and break the compile-time
graph). For coarse `Self -> Self` toggles that do **not** change the
provision list (plugins, layers), use `.when(cond, |b| ...)` with the
`config_flag(key)` / `profile_is(profile)` helpers.

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

## `Option<T>` as a first-class bean type

`Option<T>` is a distinct bean type in the graph — its `TypeId` is
`TypeId::of::<Option<T>>()`, separate from `TypeId::of::<T>()`. There is no
"soft dependency" or fallible-lookup mechanism: injecting `Option<T>` is
a **hard** dependency on the `Option<T>` slot. A producer somewhere in the
graph must register it.

**Compile-time rules:**

| Dependency type | In `Deps` (type list) | In `dependencies()` | Resolution |
|---|---|---|---|
| `T` | Yes | Yes | `ctx.get::<T>()` — panics / `MissingDependency` if absent |
| `Option<T>` | **Yes** (keyed as `Option<T>`) | **Yes** | `ctx.get::<Option<T>>()` — the stored `Option` |

Both hard (`T`) and option-typed (`Option<T>`) deps are fully checked by
the compile-time graph and by the runtime `MissingDependency` error.

### Producer pattern (the blessed way)

The producer declares `type Output = Option<T>` (inferred from the return
type by `#[producer]`) and decides `Some` / `None` internally. The slot is
always registered — the value reflects the decision:

```rust
#[producer]
async fn create_cache(
    #[config("app.cache.enabled")] enabled: bool,
) -> Option<RedisCache> {
    if enabled {
        Some(RedisCache::connect().await)
    } else {
        None
    }
}
```

### In `#[bean]` constructor params

```rust
#[bean]
impl NotificationService {
    fn new(mailer: Mailer, cache: Option<RedisCache>) -> Self {
        Self { mailer, cache }
    }
}
// dependencies() = [Mailer, Option<RedisCache>] — both hard
```

### In `#[derive(Bean)]` fields

```rust
#[derive(Clone, Bean)]
struct MyService {
    #[inject] mailer: Mailer,
    #[inject] cache: Option<RedisCache>,
}
```

### In `#[producer]` params

```rust
#[producer]
async fn create_pool(metrics: Option<MetricsCollector>) -> SqlitePool {
    // metrics is Some/None based on what Option<MetricsCollector>'s producer returned
    SqlitePool::connect("sqlite::memory:").await.unwrap()
}
```

### In the inferred HList state

There is **no hand-written state struct** (`#[derive(BeanState)]` and the
`BeanState` trait were removed). The application state is the provision list
`P` — everything `.provide()`/`.register()` — materialized as a type-level
HList by `.build_state()`. An `Option<RedisCache>` provided by a
`#[producer] -> Option<RedisCache>` is simply one more slot in `P`.

The compile-time check still routes through `AllSatisfied`: a controller's
`#[inject]` field types (its `Controller::Deps`) must all be present in `P`, or
`register_controller` is a **compile error naming the missing type**. This is
the same guarantee the old `BeanState::Requires` list gave, now derived
automatically from the controller's fields instead of a manual struct.

### Conditional beans: always register, decide inside the producer

The blessed pattern for macro consumers is to always register a
`#[producer] -> Option<T>` and let it decide `Some`/`None` internally. The
slot stays in the provision list, so any consumer that depends on `Option<T>`
compiles; the value reflects config or runtime state:

```rust
// Slot always present, value reflects config
#[producer]
async fn create_cache(#[config("cache.enabled")] on: bool) -> Option<Cache> {
    on.then(Cache::new)
}

let app = AppBuilder::new()
    .register::<CreateCache>();            // always registered
app.build_state().await                    // state type inferred from P
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
- If a hook returns `Err`, `try_build_state()` yields `BeanError::PostConstruct(String)` (and `build_state()` panics with that message).

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
- `r2e-core/src/type_list.rs` — HList state (`HCons`/`HNil`), `HasBean`, `BeanAccess` (`state.get::<T>()`), `BeanLookup` (`state.bean::<T>()`), `BuildHList`, `AllSatisfied`, `ControllerTuple`
- `r2e-core/src/builder/` — unified `register()`, `provide()`, `when()` + `config_flag()` / `profile_is()`, `with_default_bean()`/`register_override()` (last-wins override), async `build_state()` / `try_build_state()`; `RegisterController` / `RegisterControllers` extension traits (typed phase, `builder/typed.rs`)
- `r2e-macros/src/bean_attr.rs` — `#[bean]` (sync + async detection, `#[config]` param support, `Option<T>` detection, `#[consumer]` scanning + `EventSubscriber` generation, `scan_post_construct_methods` + `PostConstruct` generation)
- `r2e-macros/src/bean_derive.rs` — `#[derive(Bean)]` (`#[inject]` + `#[config]` field support, `Option<T>` detection)
- `r2e-macros/src/producer_attr.rs` — `#[producer]` macro (`Option<T>` detection)
- `r2e-macros/src/type_utils.rs` — `unwrap_option_type()` helper shared by all bean macros
- `r2e-core/src/event_subscriber.rs` — `EventSubscriber` trait (for beans with `#[consumer]` methods)
