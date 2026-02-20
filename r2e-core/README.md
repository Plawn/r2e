# r2e-core

Core runtime for the R2E web framework — AppBuilder, plugins, guards, interceptors, and compile-time dependency injection.

## Overview

`r2e-core` is the foundation crate that provides the runtime infrastructure for R2E applications. Most users should depend on the [`r2e`](../r2e) facade crate instead of using this directly.

## AppBuilder

Fluent two-phase API for assembling an application:

```rust
AppBuilder::new()
    // Phase 1: pre-state (bean registration)
    .plugin(Scheduler)                     // pre-state plugin
    .provide(my_pool.clone())              // pre-built instance
    .with_bean::<UserService>()            // sync bean
    .with_async_bean::<MyAsyncService>()   // async bean
    .with_producer::<CreatePool>()         // async producer
    .build_state::<Services, _, _>().await    // resolve bean graph → phase 2

    // Phase 2: post-state (plugins, routes, lifecycle)
    .with_config(config)
    .with(Health)
    .with(Cors::permissive())
    .with(Tracing)
    .with(ErrorHandling)
    .on_start(|state| async move { Ok(()) })
    .on_stop(|| async { })
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

`build_state()` is **async** — it resolves the bean dependency graph with topological sorting.

`build()` returns an `axum::Router`. `serve(addr)` builds, runs startup hooks, registers event consumers, starts scheduled tasks, listens, waits for shutdown (Ctrl-C / SIGTERM), then runs shutdown hooks.

## Dependency injection

Three bean traits resolved at compile time:

| Trait | Constructor | Registration |
|-------|-----------|-------------|
| `Bean` | `fn build(ctx) -> Self` | `.with_bean::<T>()` |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.with_async_bean::<T>()` |
| `Producer` | `async fn produce(ctx) -> Output` | `.with_producer::<P>()` |

`BeanContext` holds all resolved beans. `BeanRegistry` manages registration and topological resolution. Cyclic dependencies, missing dependencies, and duplicates are detected at build time via `BeanError`.

### Controller injection scopes

- `#[inject]` — app-scoped, cloned from Axum state
- `#[inject(identity)]` — request-scoped, extracted via `FromRequestParts`
- `#[config("key")]` — app-scoped, resolved from `R2eConfig`

### StatefulConstruct

`StatefulConstruct<S>` constructs a controller from state alone (no HTTP context). Auto-generated for controllers without `#[inject(identity)]` struct fields. Required by `#[consumer]` and `#[scheduled]` methods.

## Plugin system

- `Plugin` — post-state plugins installed via `.with(plugin)`
- `PreStatePlugin` — pre-state plugins installed via `.plugin(plugin)`, can provide beans to the graph
- `DeferredAction` — closure-based setup that runs after state construction (add layers, serve/shutdown hooks)

### Built-in plugins

| Plugin | Description |
|--------|-------------|
| `Health` | `GET /health` → 200 "OK" |
| `AdvancedHealth` | JSON health with liveness/readiness probes |
| `Cors::permissive()` | Permissive CORS headers |
| `Cors::custom(layer)` | Custom CORS configuration |
| `Tracing` | Request tracing via `tracing` + `tower-http` |
| `ErrorHandling` | Catches panics → JSON 500 |
| `NormalizePath` | Trailing-slash normalization |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation (UUID v4) |
| `SecureHeaders` | Security headers (HSTS, X-Frame-Options, etc.) |

## Guards

Authorization checks that run before the handler body:

- `Guard<S, I: Identity>` — post-auth guards with access to identity
- `PreAuthGuard<S>` — pre-auth guards that run before JWT extraction
- `GuardContext<I>` provides `method_name`, `controller_name`, `headers`, `uri`, `path_params`, `identity`
- `PreAuthGuardContext` provides the same without identity

```rust
struct TenantGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for TenantGuard {
    fn check(
        &self, state: &S, ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            match ctx.identity_claims() {
                Some(claims) if claims["tenant_id"].is_string() => Ok(()),
                _ => Err(AppError::Forbidden("Missing tenant".into()).into_response()),
            }
        }
    }
}
```

### Identity trait

Guards are generic over `Identity`, decoupling them from the concrete `AuthenticatedUser`:

```rust
pub trait Identity: Send + Sync {
    fn sub(&self) -> &str;
    fn roles(&self) -> &[String];
    fn email(&self) -> Option<&str> { None }
    fn claims(&self) -> Option<&serde_json::Value> { None }
}
```

`NoIdentity` is a sentinel type for when no identity is available.

## Interceptors

Cross-cutting concerns via `Interceptor<R, S>` with an `around` pattern. All calls are monomorphized for zero overhead:

```rust
impl<R: Send, S: Send + Sync> Interceptor<R, S> for MyInterceptor {
    async fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> R
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send {
        println!("before");
        let result = next().await;
        println!("after");
        result
    }
}
```

`Cacheable` trait enables response caching — built-in impls for `Json<T>` and `Result<T, E>`.

## Configuration

`R2eConfig` loads from YAML files with environment variable overlay:

```rust
let config = R2eConfig::load("dev")?; // application.yaml + application-dev.yaml + env
let db_url: String = config.get("app.db.url")?;
let timeout: i64 = config.get_or("app.timeout", 30);
```

**Resolution order:**
1. `application.yaml` (base)
2. `application-{profile}.yaml` (profile override)
3. `${...}` placeholder resolution (env vars, files via `SecretResolver`)
4. Environment variables (`APP_DB_URL` → `app.db.url`)

### Typed configuration sections

`ConfigProperties` provides strongly-typed config with compile-time validation:

```rust
#[derive(ConfigProperties)]
#[config(prefix = "app.database")]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: i64,
    #[config(default = "5")]
    pub min_connections: i64,
}
```

### Secret resolution

`SecretResolver` trait for resolving `${...}` placeholders in config values. `DefaultSecretResolver` supports:
- `${VAR_NAME}` or `${env:VAR_NAME}` — environment variables
- `${file:/path/to/secret}` — file contents

## Error handling

Built-in `AppError` with HTTP status mapping:

```rust
pub enum AppError {
    NotFound(String),      // → 404
    Unauthorized(String),  // → 401
    Forbidden(String),     // → 403
    BadRequest(String),    // → 400
    Internal(String),      // → 500
    Custom { status, body }, // → custom
}
```

Convenience type aliases:
- `ApiResult<T>` = `Result<T, AppError>`
- `JsonResult<T>` = `Result<Json<T>, AppError>`
- `StatusResult` = `Result<StatusCode, AppError>`

## Managed resources

Automatic acquire/release lifecycle for resources like database transactions:

```rust
pub trait ManagedResource<S>: Sized {
    type Error: Into<Response>;
    async fn acquire(state: &S) -> Result<Self, Self::Error>;
    async fn release(self, success: bool) -> Result<(), Self::Error>;
}
```

`success` is `true` if the handler returned `Ok`, `false` on `Err`. Error wrappers: `ManagedError` (wraps `AppError`), `ManagedErr<E>` (wraps any `E: IntoResponse`).

```rust
#[post("/")]
async fn create(&self, body: Json<User>, #[managed] tx: &mut Tx<'_, Sqlite>) -> Result<Json<User>, AppError> {
    // tx acquired before handler, committed/rolled back after
}
```

## Health checks

Simple mode with `Health` plugin, or advanced mode with custom indicators:

```rust
use r2e_core::{HealthBuilder, HealthIndicator, HealthStatus};

struct DatabaseHealth { pool: SqlitePool }

impl HealthIndicator for DatabaseHealth {
    fn name(&self) -> &str { "database" }
    async fn check(&self) -> HealthStatus {
        match sqlx::query("SELECT 1").fetch_one(&self.pool).await {
            Ok(_) => HealthStatus::Up,
            Err(e) => HealthStatus::Down(e.to_string()),
        }
    }
}

let health = HealthBuilder::new()
    .check(DatabaseHealth { pool })
    .cache_ttl(Duration::from_secs(10))
    .build();

// Exposes:
// GET /health       → aggregated JSON status
// GET /health/live  → always 200 (liveness probe)
// GET /health/ready → 200 if all checks pass, 503 if any fail
```

## SSE (Server-Sent Events)

Multi-client broadcaster for server-sent events:

```rust
use r2e_core::SseBroadcaster;

let broadcaster = SseBroadcaster::new(100); // channel capacity

// Broadcast to all subscribers
broadcaster.send("hello")?;
broadcaster.send_event("update", json!({"count": 42}).to_string())?;

// Subscribe (returns a stream for Axum SSE handler)
let subscription = broadcaster.subscribe();
```

## WebSocket (feature: `ws`)

Ergonomic wrapper around Axum WebSocket:

```rust
use r2e_core::WsStream;

let mut ws = WsStream::new(socket);
ws.send_text("hello").await?;
ws.send_json(&my_data).await?;
```

`WsBroadcaster` and `WsRooms` provide multi-client broadcast utilities.

## Validation (feature: `validation`)

`Validated<T>` extractor that deserializes JSON and validates fields:

```rust
use r2e_core::Validated;

#[post("/")]
async fn create(&self, Validated(body): Validated<CreateUserRequest>) -> Json<User> {
    // body is validated — handler only runs if all checks pass
}
```

Returns 400 with structured `ValidationErrorResponse` on failure.

## Multipart file upload (feature: `multipart`)

`TypedMultipart<T>` extractor for multipart form data:

```rust
use r2e_core::{TypedMultipart, UploadedFile};

#[derive(FromMultipart)]
struct UploadForm {
    name: String,
    file: UploadedFile,
}

#[post("/upload")]
async fn upload(&self, TypedMultipart(form): TypedMultipart<UploadForm>) -> StatusCode {
    println!("Received {} ({} bytes)", form.file.file_name.unwrap_or_default(), form.file.len());
    StatusCode::OK
}
```

## Request ID

`RequestId` extractor reads `X-Request-Id` from the request or generates a UUID v4. Propagates to the response header:

```rust
use r2e_core::RequestId;

#[get("/")]
async fn handler(&self, req_id: RequestId) -> String {
    format!("Request: {}", req_id)
}
```

Install with `.with(RequestIdPlugin)`.

## Secure headers

Adds security-related HTTP response headers:

```rust
// Default: X-Content-Type-Options, X-Frame-Options, HSTS, Referrer-Policy
.with(SecureHeaders::default())

// Custom
.with(SecureHeaders::builder()
    .hsts(true)
    .hsts_max_age(63072000)
    .frame_options("SAMEORIGIN")
    .content_security_policy("default-src 'self'")
    .build())
```

## Service components

Background services that don't handle HTTP requests:

```rust
pub trait ServiceComponent<S>: Sized {
    fn from_state(state: &S) -> Self;
    async fn start(self, shutdown: CancellationToken);
}
```

Register with `.spawn_service::<MyService>()`.

## Lifecycle hooks

```rust
AppBuilder::new()
    // ...
    .on_start(|state| async move {
        println!("Server starting");
        Ok(())
    })
    .on_stop(|| async {
        println!("Server stopped");
    })
```

`LifecycleController<T>` trait provides per-controller startup/shutdown hooks.

## Metadata

`MetaRegistry` collects type-erased metadata during controller registration. Used by `r2e-openapi` to build API specs from `RouteInfo`:

```rust
pub struct RouteInfo {
    pub path: String,
    pub method: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    // ...
}
```

## Feature flags

| Feature | Description |
|---------|-------------|
| `validation` | `Validated<T>` extractor via `validator` crate |
| `ws` | WebSocket support (`WsStream`, `WsBroadcaster`, `WsRooms`) |
| `multipart` | File upload support (`TypedMultipart`, `UploadedFile`) |

## License

Apache-2.0
