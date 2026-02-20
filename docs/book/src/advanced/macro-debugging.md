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

## What `#[derive(Controller)]` generates

Given this controller:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject] event_bus: EventBus,
}
```

The derive produces **three items**:

### 1. Metadata module `__r2e_meta_UserController`

```rust
#[doc(hidden)]
mod __r2e_meta_UserController {
    use super::*;
    pub type State = AppState;
    pub const PATH_PREFIX: Option<&str> = Some("/users");
    pub type IdentityType = r2e::NoIdentity;  // no #[inject(identity)] field

    pub fn guard_identity(_ctrl: &super::UserController) -> Option<&r2e::NoIdentity> {
        None
    }

    pub fn validate_config(_config: &r2e::config::R2eConfig) -> Vec<r2e::config::MissingKeyError> {
        Vec::new()
    }
}
```

This module is referenced by `#[routes]` through naming convention. It tells the routes macro what state type to use, what the path prefix is, and what identity type is available for guards.

### 2. Extractor struct `__R2eExtract_UserController`

```rust
#[doc(hidden)]
pub struct __R2eExtract_UserController(pub UserController);

impl r2e::http::extract::FromRequestParts<AppState> for __R2eExtract_UserController {
    type Rejection = r2e::http::response::Response;

    async fn from_request_parts(
        __parts: &mut r2e::http::header::Parts,
        __state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(UserController {
            user_service: __state.user_service.clone(),
            event_bus: __state.event_bus.clone(),
        }))
    }
}
```

Each `#[inject]` field is cloned from the corresponding field on the state struct. Identity fields (`#[inject(identity)]`) use Axum's `FromRequestParts` instead.

### 3. `StatefulConstruct` impl

```rust
impl r2e::StatefulConstruct<AppState> for UserController {
    fn from_state(__state: &AppState) -> Self {
        Self {
            user_service: __state.user_service.clone(),
            event_bus: __state.event_bus.clone(),
        }
    }
}
```

This is only generated when there are **no** `#[inject(identity)]` struct fields. It enables the controller to be used with `#[consumer]` and `#[scheduled]` methods that run outside of HTTP requests.

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
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<User>, AppError> {
        // ...
    }
}
```

### Plain handler

```rust
async fn __r2e_UserController_list(
    __R2eExtract_UserController(__ctrl): __R2eExtract_UserController,
) -> Json<Vec<User>> {
    __ctrl.list().await
}
```

The extractor constructs the controller from state, then the handler delegates to the method.

### Guarded handler (with `#[roles]`)

```rust
async fn __r2e_UserController_get_by_id(
    axum::extract::State(__state): axum::extract::State<AppState>,
    __headers: axum::http::HeaderMap,
    __uri: axum::http::Uri,
    __R2eExtract_UserController(__ctrl): __R2eExtract_UserController,
    Path(id): Path<i64>,
) -> Result<Json<User>, AppError> {
    // Guard check runs before method body
    let __identity_ref = __r2e_meta_UserController::guard_identity(&__ctrl);
    let __guard_ctx = r2e::GuardContext::new(
        "get_by_id",
        "UserController",
        &__headers,
        &__uri,
        __identity_ref,
    );
    r2e::Guard::check(&r2e::RolesGuard::new(&["admin"]), &__state, &__guard_ctx)
        .await
        .map_err(/* ... */)?;

    // Original method body
    __ctrl.get_by_id(Path(id)).await
}
```

Guarded handlers also extract `State`, `HeaderMap`, and `Uri` to build a `GuardContext`.

### `Controller<AppState>` impl

```rust
impl r2e::Controller<AppState> for UserController {
    fn routes() -> axum::Router<AppState> {
        axum::Router::new()
            .route("/users/", axum::routing::get(__r2e_UserController_list))
            .route("/users/{id}", axum::routing::get(__r2e_UserController_get_by_id))
    }

    fn register_meta(registry: &mut r2e::MetaRegistry) { /* ... */ }

    fn scheduled_tasks(_state: &AppState) -> Vec<r2e::scheduling::ScheduledTaskDef<AppState>> {
        vec![]
    }
}
```

## What `#[bean]` generates

### Sync bean

```rust
#[bean]
impl UserService {
    fn new(event_bus: EventBus) -> Self { Self { event_bus } }
}
```

Generates:

```rust
impl r2e::beans::Bean for UserService {
    type Deps = r2e::type_list::TCons<EventBus, r2e::type_list::TNil>;

    fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
        vec![(std::any::TypeId::of::<EventBus>(), std::any::type_name::<EventBus>())]
    }

    fn build(ctx: &r2e::beans::BeanContext) -> Self {
        let __arg_0: EventBus = ctx.get::<EventBus>();
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

## `#[scheduled]` and `#[consumer]`

These contribute to the `Controller` trait impl generated by `#[routes]`:

- `#[scheduled(every = 30)]` methods appear in `scheduled_tasks()` as `ScheduledTaskDef` entries
- `#[consumer(bus = "event_bus")]` methods are wired up in `register_event_consumers()`

Both require `StatefulConstruct` (i.e. no struct-level `#[inject(identity)]`).

## Debugging tips

### Common error patterns

| Error message | Cause | Fix |
|---------------|-------|-----|
| `cannot find __R2eExtract_X` | Missing `#[derive(Controller)]` on the struct | Add `#[derive(Controller)]` |
| `StatefulConstruct is not implemented` | Struct has `#[inject(identity)]` field | Use param-level `#[inject(identity)]` on individual handlers instead |
| `#[controller(state = ...)] is required` | Missing `state` in controller attribute | Add `#[controller(state = AppState)]` |
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
