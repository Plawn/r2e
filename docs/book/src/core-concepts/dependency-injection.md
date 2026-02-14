# Dependency Injection

R2E uses compile-time dependency injection — no runtime reflection, no trait objects, no service locator. All dependencies are resolved at compile time through Rust's type system.

## Three injection scopes

| Attribute | Scope | Mechanism | Cost per request |
|-----------|-------|-----------|-----------------|
| `#[inject]` | App | `state.field.clone()` | ~10-50 ns (Arc) |
| `#[inject(identity)]` | Request | `FromRequestParts` | ~10-50 us (JWT) |
| `#[config("key")]` | Config | `config.get(key)` | ~50 ns |

### `#[inject]` — App-scoped

Clones the field from the Axum state. The type must exist as a field in your state struct and implement `Clone + Send + Sync`:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject] pool: SqlitePool,
}
```

**Tip:** Wrap services in `Arc` for cheap clones (reference count increment instead of deep copy).

### `#[inject(identity)]` — Request-scoped

Extracts identity from the HTTP request (typically a JWT bearer token). The type must implement `Identity`:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
}
```

When placed on a struct field, **every** handler in the controller requires authentication. For selective auth, use [param-level identity](./controllers.md#mixed-controllers-param-level-identity).

### `#[config("key")]` — Config-scoped

Resolves a value from `R2eConfig` at request time. Supported types: `String`, `i64`, `f64`, `bool`, `Option<T>`:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[config("app.greeting")] greeting: String,
    #[config("app.max-retries")] max_retries: i64,
    #[config("app.optional-key")] maybe: Option<String>,
}
```

Missing required config keys panic at request time with a message including the environment variable equivalent (e.g., `APP_GREETING`).

## StatefulConstruct

When a controller has no struct-level `#[inject(identity)]` fields, R2E generates a `StatefulConstruct<S>` implementation. This allows constructing the controller from state alone (without an HTTP request), which is required for:

- **Event consumers** (`#[consumer]`) — handle events outside HTTP context
- **Scheduled tasks** (`#[scheduled]`) — run background jobs

Controllers with struct-level `#[inject(identity)]` fields do **not** get `StatefulConstruct` — attempting to use them for consumers or scheduled tasks produces a compile error.

The [mixed controller pattern](./controllers.md#mixed-controllers-param-level-identity) (param-level `#[inject(identity)]`) preserves `StatefulConstruct` while still supporting authentication on individual endpoints.
