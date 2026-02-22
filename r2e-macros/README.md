# r2e-macros

Procedural macros for the R2E framework — `#[derive(Controller)]`, `#[routes]`, `#[bean]`, and `#[producer]`.

## Overview

This proc-macro crate generates all the Axum boilerplate at compile time with zero runtime reflection. Most users should depend on the [`r2e`](../r2e) facade crate, which re-exports these macros automatically.

## Macros

### `#[derive(Controller)]`

Generates controller metadata, Axum extractor, and `StatefulConstruct` impl:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}
```

**Generated items:**
- `mod __r2e_meta_UserController` — type aliases and constants
- `struct __R2eExtract_UserController` — `FromRequestParts` extractor
- `impl StatefulConstruct` — when no `#[inject(identity)]` struct fields

### `#[routes]`

Generates Axum handler functions and `Controller` trait impl:

```rust
#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> { ... }

    #[post("/")]
    #[roles("admin")]
    async fn create(&self, body: Json<CreateUser>) -> Result<Json<User>, HttpError> { ... }
}
```

**Supported attributes on methods:**
- `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` — HTTP routes
- `#[roles("...")]` — role-based access control
- `#[guard(MyGuard)]` — custom post-auth guard
- `#[pre_guard(MyGuard)]` — custom pre-auth guard
- `#[intercept(Logged::info())]` — interceptors
- `#[transactional]` — transaction wrapping
- `#[managed]` — managed resource lifecycle (on parameters)
- `#[consumer(bus = "field")]` — event consumer
- `#[scheduled(every = 30)]` — scheduled task
- `#[middleware(fn)]` — Tower middleware

### `#[bean]`

Auto-detects sync vs async constructors:

```rust
#[bean]
impl UserService {
    fn new(repo: UserRepo) -> Self { Self { repo } }
}

#[bean]
impl DbService {
    async fn new(#[config("db.url")] url: String) -> Self { ... }
}
```

### `#[producer]`

Generates a factory struct for types you don't own:

```rust
#[producer]
async fn create_pool(#[config("app.db.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
// Generates: struct CreatePool; impl Producer for CreatePool { type Output = SqlitePool; }
```

### `#[derive(Bean)]`

Derive-based bean with field injection:

```rust
#[derive(Clone, Bean)]
struct MyService {
    #[inject] event_bus: EventBus,
    #[config("app.name")] name: String,
}
```

## Crate path resolution

The macros use `proc-macro-crate` to detect whether the downstream crate depends on `r2e` (facade) or `r2e-core` directly, generating correct paths like `::r2e::` or `::r2e_core::` accordingly.

## License

Apache-2.0
