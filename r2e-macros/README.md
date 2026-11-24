# r2e-macros

Procedural macros for the R2E framework — `#[controller]`, `#[routes]`, `#[bean]`, `#[producer]`, `#[module]`, the route/MCP/lifecycle attributes, the derives, and `#[r2e::main]` / `#[r2e::test]`.

## Overview

This proc-macro crate generates all the Axum boilerplate at compile time with zero runtime reflection. Most users should depend on the [`r2e`](../r2e) facade crate, which re-exports these macros automatically.

## Macro reference

Every macro exported by this crate (re-exported by the `r2e` facade; most are in
`r2e::prelude`). Attributes marked *marker* do nothing on their own — they are
read by the enclosing `#[routes]` / `#[bean]` / `#[controller]` / `#[r2e::test_suite]`
expansion and fail to compile anywhere else. The rustdoc of each macro is the
authoritative reference for its arguments.

### Types, beans and modules

| Macro | Applies to | Purpose |
|-------|-----------|---------|
| `#[controller(path = "...")]` | struct | Controller core + request-data extractor + per-request façade + `ContextConstruct` |
| `#[routes]` | impl block | Generates the HTTP handlers / MCP members and the `Controller` impl (one type = one port) |
| `#[grpc_routes(...)]` | impl block | Wires a tonic-generated service trait into R2E |
| `#[bean]` | impl block | `Bean` / `AsyncBean` impl from the constructor's parameters |
| `#[producer]` | free fn | `Producer` impl for types you don't own (`#[producer(start)]` for background services) |
| `#[module(...)]` | struct | `FeatureModule` impl: providers, controllers, exports, imports, plugins |
| `#[derive(Bean)]` | struct | Field-injected bean (`#[inject]`, `#[config]`, `#[config_section]`, `#[live_config]`, `#[default]`) |
| `#[derive(ProvideBundle)]` | struct | Provision bundle for a single `provide_all(..)` call |
| `#[derive(DecoratorBean)]` | struct | `DecoratorSpec` plumbing for guards/interceptors with bean deps |
| `#[derive(BackgroundService)]` | struct | `ServiceComponent` impl, registered with `spawn_service` |
| `#[derive(ConfigProperties)]` | struct | Strongly-typed configuration section (`#[config(...)]` on fields) |
| `#[derive(FromConfigValue)]` | type | `FromConfigValue` impl via serde deserialization |
| `#[derive(Cacheable)]` | type | `Cacheable` impl through the `r2e::json` codec |
| `#[derive(ApiError)]` | enum | HTTP error type (`#[error(status = ..., message = ...)]`, `#[from]`) |
| `#[derive(Params)]` | struct | Aggregates path / query / header parameters (`#[param]`, `#[query]`, `#[header]`, `#[params]`) |
| `#[derive(FromMultipart)]` | struct | Typed multipart form extraction (feature `multipart`) |
| `#[derive(ObjectParams)]` | struct | Marks an MCP `Params<T>` type as an object-shaped argument structure |

Field attributes understood by `#[controller]`, `#[derive(Bean)]` and
`#[derive(DecoratorBean)]`: `#[inject]`, `#[inject(identity)]` / `#[inject(request)]`
(controllers only), `#[config("key")]`, `#[config_section(prefix = "...")]`,
`#[live_config("key")]`.

### Route methods (inside `#[routes]`)

| Macro | Purpose |
|-------|---------|
| `#[get]` `#[post]` `#[put]` `#[delete]` `#[patch]` | HTTP route for one method |
| `#[any("/...")]` | Route matching every HTTP method (proxy / catch-all with `{*wildcard}`) |
| `#[fallback]` | Controller-scoped catch-all for unmatched requests |
| `#[sse("/...")]` | Server-Sent Events endpoint |
| `#[ws("/...")]` | WebSocket endpoint (feature `ws`) |
| `#[roles("...")]` / `#[all_roles("...")]` | Role check, OR / AND semantics |
| `#[anonymous]` | Opt a route out of struct-level identity (fail-closed auth) |
| `#[guard(expr)]` / `#[pre_guard(expr)]` | Post-auth / pre-auth guard |
| `#[intercept(expr)]` | Interceptor on a method or a whole impl block |
| `#[middleware(fn)]` | Middleware function on one route |
| `#[layer(expr)]` | Tower layer on one route |
| `#[status(N)]` | Documented success status code |
| `#[returns(T)]` | Explicit response type for OpenAPI |
| `#[raw]` | *marker* on a parameter: raw extractor (documentation only) |
| `#[managed]` | *marker* on a parameter: `ManagedResource` acquire/release lifecycle |
| `#[request_helper]` | Emit a helper method on the per-request façade instead of the core |

### Background work and lifecycle (on `#[bean]` impls and `#[routes]` controllers)

| Macro | Purpose |
|-------|---------|
| `#[consumer(bus = "field")]` | Event consumer on the named bus field |
| `#[scheduled(every = N \| cron = "..." \| schedule = "method", ...)]` | Scheduled task (`initial_delay`, `name`, `overlap`, `skip_if`) |
| `#[async_exec]` | Submit the method body to the `PoolExecutor` |
| `#[post_construct]` | Hook after the bean graph is built (`Err` aborts startup) |
| `#[on_start(order = N)]` | Startup observer once the graph and controller cores exist (`Err` aborts) |
| `#[pre_destroy]` | Graceful-shutdown hook, reverse order (`Err` is logged) |

### MCP members (inside `#[routes]`, feature `mcp`)

| Macro | Purpose |
|-------|---------|
| `#[tool]` | *marker*: MCP tool |
| `#[resource(uri = "...")]` | *marker*: MCP resource or resource template |
| `#[prompt]` | *marker*: MCP prompt |
| `#[completion]` | *marker*: Completion provider for a prompt argument / resource-template variable |
| `#[method(name = "ns/verb")]` | *marker*: MCP extension JSON-RPC method on the same endpoint |

### Entry points and tests

| Macro | Purpose |
|-------|---------|
| `#[r2e::main]` | `async fn main()` entry point on the R2E runtime (`start_paused`, … — see rustdoc) |
| `#[r2e::test]` | Async test on the R2E runtime (`app = MyApp` boots the app) — use it instead of `#[tokio::test]` |
| `#[r2e::test_suite]` | One Cargo test per `#[case]` method, sharing the suite instance |
| `#[case]` | *marker*: a test case inside `#[r2e::test_suite]` |
| `#[before_all]` `#[before_each]` `#[after_each]` `#[after_all]` | *marker*: suite lifecycle hooks (camelCase aliases `beforeAll`, … also exported) |

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
- `struct __R2eRequestData_UserController` — `RequestData<S>` extractor for the request-scoped values (identity + `#[inject(request)]`), failing with a `Rejection`
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
    #[inject] event_bus: LocalEventBus,
    #[config("app.name")] name: String,
}
```

## Crate path resolution

The macros use `proc-macro-crate` to detect whether the downstream crate depends on `r2e` (facade) or `r2e-core` directly, generating correct paths like `::r2e::` or `::r2e_core::` accordingly.

## License

Apache-2.0
