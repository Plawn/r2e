# r2e-macros

Procedural macros for the R2E framework — `#[controller]`, `#[routes]`, `#[bean]`, and `#[producer]`.

## Overview

This proc-macro crate generates all the Axum boilerplate at compile time with zero runtime reflection. Most users should depend on the [`r2e`](../r2e) facade crate, which re-exports these macros automatically.

## Macros

### `#[controller(...)]`

A transforming attribute on the struct. It rewrites the struct into a physical
*core* (request-scoped fields stripped out, built once into an `Arc`) and
generates the request-data extractor, the per-request façade, metadata, and
`ContextConstruct`. There is no `state = ...` argument — `#[inject]` fields are
resolved from the bean graph by type:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,        // request-scoped (auth)
    #[inject(request)] tenant: TenantId,                // request-scoped (generic)
    #[config("app.greeting")] greeting: String,
}
```

**Generated items:**
- the controller **core** (struct with request-scoped fields stripped)
- `mod __r2e_meta_UserController` — type aliases, constants, `guard_identity`, `bind_request`, `validate_config`
- `struct __R2eRequestData_UserController` — `FromRequestParts` extractor for the request-scoped values (identity + `#[inject(request)]`)
- `struct __R2eRequest_UserController` — the per-request façade, `Deref<Target = core>`; route methods run here
- `impl ContextConstruct` — always (the core builds from the resolved `BeanContext`, fetching each `#[inject]` field by type)

A missing bean for an `#[inject]` field is a **compile error naming the type**,
checked via `Controller::Deps` / `AllSatisfied` at `register_controller`. The
generated `Controller<S, W>` impl is generic over the state `S`.

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
- `#[any("/...")]` — any-method route (proxy/catch-all with `{*wildcard}` paths)
- `#[fallback]` — controller-scoped catch-all for unmatched requests
- `#[roles("...")]`, `#[all_roles("...")]` — role-based access control (OR / AND)
- `#[anonymous]` — opt a route out of struct-level identity (fail-closed auth)
- `#[guard(MyGuard)]` — custom post-auth guard
- `#[pre_guard(MyGuard)]` — custom pre-auth guard
- `#[intercept(Logged::info())]` — interceptors
- `#[managed]` — managed resource lifecycle (on parameters)
- `#[async_exec]` — submit the method body to a `PoolExecutor`
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
