# Dependency Injection

R2E uses compile-time dependency injection — no runtime reflection, no trait objects, no service locator. All dependencies are resolved at compile time through Rust's type system.

## Four injection scopes

Two scopes are **app-scoped** — resolved once when the controller core is built
at router-registration time — and two are **request-scoped** — extracted per
request into a small stack façade that holds an `Arc` to the core:

| Attribute | Scope | Lives on | Mechanism | Resolved |
|-----------|-------|----------|-----------|----------|
| `#[inject]` | App | Core | `ctx.get::<T>()` (by type) | Once, at build |
| `#[config("key")]` | App | Core | `config.get(key)` | Once, at build |
| `#[inject(identity)]` | Request | Façade | `FromRequestParts` (auth identity) | Per request |
| `#[inject(request)]` | Request | Façade | `FromRequestParts` (any value) | Per request |

The app-scoped fields are resolved a single time; the only per-request cost for a
controller is one `Arc` clone of the core plus the `FromRequestParts` extraction
of its request-scoped fields (which, for identity, includes JWT verification).

### `#[inject]` — App-scoped

Resolves the field from the bean graph **by type**. The type must be present in
the graph (via `.provide` or `.register`) and implement `Clone + Send + Sync`. A
missing bean is a **compile error naming the type**:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject] pool: SqlitePool,
}
```

**Tip:** Wrap services in `Arc` for cheap clones (reference count increment instead of deep copy).

### `#[inject(identity)]` — Request-scoped (auth)

Extracts identity from the HTTP request (typically a JWT bearer token). The type must implement `Identity` and drives guards and `#[roles(...)]`:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
}
```

When placed on a struct field, **every** handler in the controller requires authentication. For selective auth, use [param-level identity](./controllers.md#mixed-controllers-param-level-identity). `Option<AuthenticatedUser>` makes the identity optional.

### `#[inject(request)]` — Request-scoped (generic)

For request-scoped values that are **not** the auth identity, use `#[inject(request)]`. Any type implementing `FromRequestParts<S>` (generic over the state) qualifies — for example a tenant id, a correlation/trace context, or a request-scoped handle:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject(request)] tenant: TenantId,
    #[inject(request)] trace: Option<TraceContext>,
}
```

Like identity, these live on the per-request façade and are isolated per request. `Option<T>` is supported. Unlike identity, `#[inject(request)]` does not participate in guards/roles. (Current limitation: `#[inject(request)]` fields are not modeled in OpenAPI yet.)

### `#[config("key")]` — App-scoped config

Resolves a value from `R2eConfig` when the core is built. Supported types: `String`, `i64`, `f64`, `bool`, `Option<T>`:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[config("app.greeting")] greeting: String,
    #[config("app.max-retries")] max_retries: i64,
    #[config("app.optional-key")] maybe: Option<String>,
}
```

Missing required config keys fail with a message including the environment variable equivalent (e.g., `APP_GREETING`).

## ContextConstruct

R2E always generates a `ContextConstruct` implementation for the controller core.
It builds the core **from the resolved bean graph by type** — `ctx.get::<T>()`
per `#[inject]` field, plus `ctx.get::<R2eConfig>()` for `#[config]` fields:

```rust
impl ContextConstruct for UserController {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            user_service: ctx.get::<UserService>(),
            greeting:     ctx.get::<R2eConfig>().get::<String>("app.greeting").unwrap(),
        }
    }
}
```

Because request-scoped fields (`#[inject(identity)]` and `#[inject(request)]`)
are stripped out of the core and only ever live on the per-request façade, the
core can always be constructed from the context alone (without an HTTP request).
This is required for:

- **Event consumers** (`#[consumer]`) — handle events outside HTTP context
- **Scheduled tasks** (`#[scheduled]`) — run background jobs

Consumers and scheduled methods run on the core and therefore **cannot** access
request identity. A controller can freely combine struct-level identity for its
HTTP routes with `#[consumer]`/`#[scheduled]` methods that use only core fields.

The [mixed controller pattern](./controllers.md#mixed-controllers-param-level-identity) (param-level `#[inject(identity)]`) is still recommended for mixed public/protected controllers, since it makes request scope explicit per endpoint.
