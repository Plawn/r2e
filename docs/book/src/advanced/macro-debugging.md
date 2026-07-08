# Macro Debugging with `cargo expand`

R2E relies heavily on proc macros to generate boilerplate. When things go wrong, seeing the generated code is invaluable. `cargo expand` shows you exactly what the macros produce.

## Setup

Install `cargo-expand`:

```bash
cargo install cargo-expand
```

Usage:

```bash
# Expand an entire crate
cargo expand -p example-app

# Expand a single module
cargo expand -p example-app controllers::user_controller

# Filter for R2E-generated items
cargo expand -p example-app 2>/dev/null | grep "__r2e_"
```

## What `#[controller]` generates

Given this controller:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject] event_bus: LocalEventBus,
}
```

`#[controller]` is a **transforming attribute** — it rewrites the struct into a
physical *core* (request-scoped fields stripped out) and produces four hidden
items around it:

### 1. The controller core

The emitted struct keeps only app-scoped fields (`#[inject]` and `#[config]`).
Any `#[inject(identity)]` / `#[inject(request)]` fields are removed — they only
ever exist on the per-request façade (below). The core is built **once** when the
router is registered and shared as an `Arc<UserController>`.

### 2. Metadata module `__r2e_meta_UserController`

```rust
#[doc(hidden)]
mod __r2e_meta_UserController {
    use super::*;
    pub const PATH_PREFIX: Option<&str> = Some("/users");
    pub const HAS_STRUCT_IDENTITY: bool = false;   // no #[inject(identity)] field
    pub type IdentityType = r2e::NoIdentity;

    // Reads the identity off the request façade (never the core).
    pub fn guard_identity(_facade: &super::__R2eRequest_UserController) -> Option<&r2e::NoIdentity> {
        None
    }

    // Moves request-scoped values + the core Arc into the façade. Generic over
    // the per-field marker tuple `__M` so `#[routes]` can thread it as one
    // opaque inferred generic.
    pub fn bind_request<__M>(
        core: std::sync::Arc<super::UserController>,
        data: super::__R2eRequestData_UserController<__M>,
    ) -> super::__R2eRequest_UserController { /* ... */ }

    pub fn validate_config(_config: &r2e::config::R2eConfig) -> Vec<r2e::config::MissingKeyError> {
        Vec::new()
    }
}
```

This module is referenced by `#[routes]` through naming convention. It carries the
path prefix, whether the controller has struct-level identity, what identity type
is available for guards, and how to bind the per-request façade. There is **no
`State` alias** — the state type is inferred (the HList of provisions), so the
generated `Controller` impl and the request-data extractor are generic over it.

### 3. Request-data extractor `__R2eRequestData_UserController`

```rust
#[doc(hidden)]
pub struct __R2eRequestData_UserController<__M> {
    // One field per request-scoped (#[inject(identity)] / #[inject(request)]) field.
    // Marker-only (zero-sized) here, since UserController has none.
    __markers: core::marker::PhantomData<__M>,
}

// State-generic: works with any inferred HList state `S`. Each request-scoped
// field is extracted through `FromRequestPartsVia<S, M>`, which lets a
// bean-backed extractor park its `HasBean` index witness in the marker `M`.
impl<S> r2e::http::extract::FromRequestParts<S> for __R2eRequestData_UserController<()>
where
    S: Send + Sync,
{
    type Rejection = r2e::http::response::Response;

    async fn from_request_parts(
        __parts: &mut r2e::http::header::Parts,
        __state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self { __markers: core::marker::PhantomData })
    }
}
```

This is the only per-request extraction. When the controller has no request-scoped
fields the extractor is marker-only and infallible (implemented for `__M = ()`).
Plain axum `FromRequestParts<S>` extractors reach this path through the blanket
`ViaAxum` bridge; R2E-owned extractors (like `AuthenticatedUser`) go through
`FromRequestPartsVia` / `BeanExtract<T, I>`.

### 4. Request façade `__R2eRequest_UserController` + `Deref`

```rust
#[doc(hidden)]
pub struct __R2eRequest_UserController {
    __core: std::sync::Arc<UserController>,
    // plus one field per request-scoped controller field
}

impl core::ops::Deref for __R2eRequest_UserController {
    type Target = UserController;
    fn deref(&self) -> &UserController { &self.__core }
}
```

Route methods run on this façade. `self.user_service` resolves to the core through `Deref`; `self.user` (if present) is a direct façade field.

### 5. `ContextConstruct` impl

```rust
impl r2e::ContextConstruct for UserController {
    // Unique #[inject] types (+ R2eConfig when #[config] fields exist),
    // checked against the state's provision list via AllSatisfied at
    // register_controller() — a missing bean is a compile error naming the type.
    type Deps = r2e::type_list::TCons<
        UserService,
        r2e::type_list::TCons<LocalEventBus, r2e::type_list::TNil>,
    >;

    // Each #[inject] field is pulled BY TYPE from the resolved bean graph.
    fn from_context(__ctx: &r2e::beans::BeanContext) -> Self {
        Self {
            user_service: __ctx.get::<UserService>(),
            event_bus: __ctx.get::<LocalEventBus>(),
        }
    }
}
```

This replaces the old `StatefulConstruct::from_state` (which cloned fields from a
hand-written state struct by name). It is **always** generated (the core never
holds request-scoped fields), builds the core once from the `BeanContext` by type,
and enables the controller to be used with `#[consumer]` and `#[scheduled]`
methods that run outside of HTTP requests. `#[config("key")]` fields resolve from
the `R2eConfig` bean (`__ctx.get::<R2eConfig>().get::<T>("key")`).

## What `#[routes]` generates

Given:

```rust
#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[get("/{id}")]
    #[roles("admin")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<User>, HttpError> {
        // ...
    }
}
```

### Plain handler

Route methods are emitted on the façade (`impl __R2eRequest_UserController`), and
each route is registered as a closure that captures the core `Arc`, extracts the
request data, and binds the façade:

```rust
// Method moved onto the façade.
impl __R2eRequest_UserController {
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)  // user_service via Deref to the core
    }
}

// Route closure (one Arc clone of the core captured once, cloned per request).
{
    let core = core.clone();
    move |__data: __R2eRequestData_UserController| {
        let core = core.clone();
        async move {
            let __ctrl = __r2e_meta_UserController::bind_request(core, __data);
            __ctrl.list().await
        }
    }
}
```

The closure binds the façade from the captured core `Arc` and the extracted request data, then delegates to the method.

### Guarded handler (with `#[roles]`)

Guards are **built once at registration** from the resolved `BeanContext` (via
`DecoratorSpec::build`) and captured by the route closure in a decorator bundle
(`__deco`) — one `Arc` per route, no state access at request time:

```rust
move |
    __headers: axum::http::HeaderMap,
    __uri: axum::http::Uri,
    __data: __R2eRequestData_UserController<()>,
    Path(id): Path<i64>,
| {
    let core = core.clone();
    let __deco = __deco.clone();     // prebuilt guards/interceptors
    async move {
        let __ctrl = __r2e_meta_UserController::bind_request(core, __data);

        // Guard check runs before method body; identity is read off the façade.
        let __identity_ref = __r2e_meta_UserController::guard_identity(&__ctrl);
        let __guard_ctx = r2e::GuardContext::new(
            "get_by_id",
            "UserController",
            &__headers,
            &__uri,
            __identity_ref,
        );
        // The guard was built at registration; check() takes no state.
        r2e::Guard::check(&__deco.__g0, &__guard_ctx)
            .await
            .map_err(/* ... */)?;

        // Original method body, on the façade.
        __ctrl.get_by_id(Path(id)).await
    }
}
```

Guarded handlers extract `HeaderMap` and `Uri` to build a `GuardContext`.
`guard_identity` reads the identity directly from the façade. The guard itself
(`__deco.__g0`) was constructed once at registration, so there is no `State`
extraction and no per-request DI.

### `Controller<S, W>` impl

The generated impl is **generic over the state** `S` (the inferred HList) and over
an opaque witness carrier `W` that parks the request-data extraction markers. User
code never names `S` or `W` — `register_controller()` infers both:

```rust
impl<S, W> r2e::Controller<S, W> for UserController
where
    S: Clone + Send + Sync + 'static + r2e::type_list::BeanLookup,
    // ... plus the inferred `HasBean` bounds carried by W
{
    // #[inject] types PLUS every guard/interceptor spec's `Deps`, folded into
    // one list and checked against S's provisions via AllSatisfied — so a bean
    // a guard needs is a compile error here too.
    type Deps = /* ContextConstruct::Deps ++ each DecoratorSpec::Deps */;

    // Build the core once from the resolved bean graph.
    fn construct(_state: &S, ctx: &r2e::beans::BeanContext) -> Self {
        <Self as r2e::ContextConstruct>::from_context(ctx)
    }

    fn routes(
        state: &S,
        core: std::sync::Arc<Self>,
        ctx: &r2e::beans::BeanContext,   // resolved graph — guards/interceptors are built here
    ) -> axum::Router<S> {
        // Guards and interceptors are built ONCE from `ctx` via
        // `<Spec as DecoratorSpec>::build(expr, ctx)`, then captured by the
        // route closures (see the guarded-handler shape above).
        axum::Router::new()
            // Each generated closure captures core.clone() (and any built decorators).
            .route("/users/", axum::routing::get(/* generated closure */))
            .route("/users/{id}", axum::routing::get(/* generated closure */))
    }

    fn register_meta(registry: &mut r2e::MetaRegistry) { /* ... */ }

    // Consumers and scheduled tasks receive the same core Arc.
}
```

Registration itself lives on the `RegisterController` / `RegisterControllers`
extension traits (in the prelude), called on the built app after
`build_state().await`.

## What `#[bean]` generates

### Sync bean

```rust
#[bean]
impl UserService {
    fn new(event_bus: LocalEventBus) -> Self { Self { event_bus } }
}
```

Generates:

```rust
impl r2e::beans::Bean for UserService {
    type Deps = r2e::type_list::TCons<LocalEventBus, r2e::type_list::TNil>;

    fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
        vec![(std::any::TypeId::of::<LocalEventBus>(), std::any::type_name::<LocalEventBus>())]
    }

    fn build(ctx: &r2e::beans::BeanContext) -> Self {
        let __arg_0: LocalEventBus = ctx.get::<LocalEventBus>();
        UserService::new(__arg_0)
    }
}
```

### Async bean

```rust
#[bean]
impl DbService {
    async fn new(pool: SqlitePool) -> Self { Self { pool } }
}
```

Generates `impl AsyncBean` with `type Deps = TCons<SqlitePool, TNil>` and an `async fn build(ctx)` instead.

## What `#[producer]` generates

```rust
#[producer]
async fn create_pool(#[config("app.db.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
```

Generates:

```rust
// Original function (with #[config] stripped)
async fn create_pool(url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}

// Generated struct
pub struct CreatePool;

impl r2e::beans::Producer for CreatePool {
    type Output = SqlitePool;
    type Deps = r2e::type_list::TCons<r2e::config::R2eConfig, r2e::type_list::TNil>;

    fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
        vec![(std::any::TypeId::of::<r2e::config::R2eConfig>(), /* ... */)]
    }

    async fn produce(ctx: &r2e::beans::BeanContext) -> Self::Output {
        let __r2e_config: r2e::config::R2eConfig = ctx.get::<r2e::config::R2eConfig>();
        let __arg_0: String = __r2e_config.get::<String>("app.db.url").unwrap_or_else(|_| {
            panic!("Configuration error in producer `CreatePool`: key 'app.db.url' ...")
        });
        create_pool(__arg_0).await
    }
}
```

The function name is converted to PascalCase for the struct name (`create_pool` -> `CreatePool`).

### `#[producer] -> Option<T>` (conditional availability)

When the user function returns `Option<T>`, the `Output` associated type is
`Option<T>` — the whole type, not the inner `T`. The `Option<T>` slot is
registered under `TypeId::of::<Option<T>>()`, and consumers hard-depend on
`Option<T>`:

```rust
#[producer]
async fn create_cache(#[config("cache.enabled")] on: bool) -> Option<RedisCache> {
    if on { Some(RedisCache::new().await) } else { None }
}

// Generates:
impl r2e::beans::Producer for CreateCache {
    type Output = Option<RedisCache>;  // the WHOLE Option, not RedisCache
    // ...
    async fn produce(ctx: &r2e::beans::BeanContext) -> Self::Output {
        let __r2e_config = ctx.get::<r2e::config::R2eConfig>();
        let __arg_0: bool = __r2e_config.get::<bool>("cache.enabled").unwrap_or_else(/* ... */);
        create_cache(__arg_0).await  // returns Option<RedisCache> verbatim
    }
}
```

Consumers declare `Option<RedisCache>` as a hard dependency (e.g. a
`#[bean] fn new(cache: Option<RedisCache>)` constructor param).

## `#[scheduled]` and `#[consumer]`

These contribute to the `Controller` trait impl generated by `#[routes]`:

- `#[scheduled(every = 30)]` methods appear in `scheduled_tasks()` as `ScheduledTaskDef` entries
- `#[consumer(bus = "event_bus")]` methods are wired up in `register_event_consumers()`

Both run on the controller core built via `ContextConstruct` (always available) and therefore cannot access request identity. They coexist with struct-level `#[inject(identity)]`, which only affects the controller's HTTP routes.

## Debugging tips

### Common error patterns

| Error message | Cause | Fix |
|---------------|-------|-----|
| `cannot find __R2eRequest_X` / `__r2e_meta_X` | Missing `#[controller(...)]` on the struct | Add `#[controller]` |
| `#[routes]` cannot find the controller metadata | `#[routes]` impl block on a struct without `#[controller(...)]` | Add `#[controller(...)]` to the struct |
| `` `T` cannot be constructed from the bean context `` | `#[routes]` / `#[grpc_routes]` on a struct without `#[controller]` | Add `#[controller]` to the struct |
| `` the trait bound `S: HasBean<T, _>` is not satisfied `` (bean `T` is not registered) | A `#[inject] T` field whose type was never registered as a bean | `.register::<T>()` or `.provide(T)` before `build_state()` |
| `every controller field must be annotated` | Field without `#[inject]`, `#[config]`, etc. | Annotate the field with one of the supported attributes |

### Filtering expanded output

The expanded output can be very long. Filter for R2E-generated items:

```bash
# Find all generated handler functions
cargo expand -p my-app 2>/dev/null | grep "fn __r2e_"

# Find all generated modules
cargo expand -p my-app 2>/dev/null | grep "mod __r2e_meta_"

# Find all Controller trait impls
cargo expand -p my-app 2>/dev/null | grep "impl.*Controller.*for"
```

### Expand a single controller

If you know your controller is in `src/controllers/user_controller.rs`:

```bash
cargo expand -p my-app controllers::user_controller
```

This limits output to just that module, making it much easier to read.
