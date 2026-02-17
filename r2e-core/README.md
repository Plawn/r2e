# r2e-core

Core runtime for the R2E web framework — AppBuilder, plugins, guards, interceptors, and compile-time dependency injection.

## Overview

`r2e-core` is the foundation crate that provides the runtime infrastructure for R2E applications. Most users should depend on the [`r2e`](../r2e) facade crate instead of using this directly.

## Key components

### AppBuilder

Fluent API for assembling an application:

```rust
AppBuilder::new()
    .with_bean::<UserService>()
    .build_state::<Services, _>()
    .await
    .with(Health)
    .with(Cors::permissive())
    .with(Tracing)
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

### Dependency injection

Three bean traits resolved at compile time:

| Trait | Constructor | Registration |
|-------|-----------|-------------|
| `Bean` | `fn build(ctx) -> Self` | `.with_bean::<T>()` |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.with_async_bean::<T>()` |
| `Producer` | `async fn produce(ctx) -> Output` | `.with_producer::<P>()` |

### Controller injection scopes

- `#[inject]` — app-scoped, cloned from Axum state
- `#[inject(identity)]` — request-scoped, extracted via `FromRequestParts`
- `#[config("key")]` — app-scoped, resolved from `R2eConfig`

### Plugin system

- `Plugin` — post-state plugins installed via `.with(plugin)`
- `PreStatePlugin` — pre-state plugins installed via `.plugin(plugin)`
- Built-in plugins: `Health`, `Cors`, `Tracing`, `ErrorHandling`, `DevReload`, `SecureHeaders`, `RequestIdPlugin`

### Guards

- `Guard<S, I>` — post-auth guards (have access to identity)
- `PreAuthGuard<S>` — pre-auth guards (run before JWT extraction)
- `RolesGuard` — built-in role-based access control

### Interceptors

Cross-cutting concerns via the `Interceptor<R>` trait with an `around` pattern. All calls are monomorphized for zero overhead.

### Configuration

`R2eConfig` loads from YAML files with environment variable overlay:

```rust
let config = R2eConfig::load("dev"); // application.yaml + application-dev.yaml + env
let db_url: String = config.get("app.db.url")?;
```

### Managed resources

Automatic lifecycle management for resources like database transactions:

```rust
#[post("/")]
async fn create(&self, body: Json<User>, #[managed] tx: &mut Tx<'_, Sqlite>) -> Result<Json<User>, AppError> {
    // tx is acquired before, committed/rolled back after
}
```

## Feature flags

| Feature | Description |
|---------|-------------|
| `validation` | `Validated<T>` extractor via `validator` |
| `ws` | WebSocket support |
| `multipart` | File upload support |

## License

Apache-2.0
