# R2E — Escape Hatches: Proposal & Implementation Ideas

> How to keep the framework opinionated while giving developers an exit when they need one.

## Why escape hatches matter

An opinionated framework is a productivity multiplier — until you hit a case it doesn't cover. At that point, the developer faces a choice: fight the framework, or rewrite outside of it. Escape hatches eliminate this dilemma by letting you **drop down one abstraction level** without leaving the framework.

Quarkus, Spring Boot, and NestJS all succeed because they provide clean boundaries between "the happy path" and "raw access." R2E should aim for the same.

---

## 1. Raw Axum Router Mounting

### Problem

Today, all HTTP endpoints go through `#[derive(Controller)]` + `#[routes]`. If a developer needs a handler that doesn't fit the controller model (e.g. a streaming response, a proxy, a low-level WebSocket upgrade), there's no obvious way to add it.

### Proposed API

```rust
use axum::{Router, routing::get, response::IntoResponse};

async fn raw_handler() -> impl IntoResponse {
    "This is a raw Axum handler"
}

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .register_controller::<UserController>()
    // Escape hatch: mount a raw Axum router alongside controllers
    .merge_router(
        Router::new()
            .route("/raw/stream", get(raw_handler))
            .route("/raw/proxy", get(proxy_handler))
    )
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Implementation notes

- `merge_router` would call `axum::Router::merge` internally, which Axum already supports natively.
- The raw router should still benefit from global plugins (Tracing, ErrorHandling, CORS) since those are tower layers applied at the top level.
- Document clearly: "Raw routes do NOT get controller-level DI, interceptors, or guards. Use this for cases where you need full control."

---

## 2. Programmatic DI / Manual Bean Registration

### Problem

`#[bean]` and `#[inject]` work great for static, known-at-compile-time dependency graphs. But sometimes you need:

- A service whose construction depends on runtime config (e.g. "use Postgres in prod, SQLite in test").
- A mock service in tests.
- A service built from an external factory.

### Proposed API

```rust
// Register a bean with a factory closure
AppBuilder::new()
    .with_bean::<UserService>()
    // Escape hatch: register a bean programmatically
    .with_bean_factory(|config: &R2eConfig| -> EmailService {
        if config.get::<bool>("app.email.mock").unwrap_or(false) {
            EmailService::mock()
        } else {
            EmailService::smtp(config.get::<String>("app.email.host").unwrap())
        }
    })
    .build_state::<AppState, _, _>()
    .await
```

### For testing: bean override

```rust
// In tests — override a bean after initial construction
let app = TestApp::from_builder(
    AppBuilder::new()
        .with_bean::<UserService>()
        // Override: replace UserService with a mock
        .override_bean(MockUserService::new())
        .build_state::<AppState, _, _>()
        .await
        .register_controller::<UserController>(),
);
```

### Implementation notes

- This is analogous to Quarkus' `@Produces` + `@Alternative` or Spring's `@Bean` + `@Primary`.
- Since DI is compile-time, `override_bean` could work via a trait bound: if `MockUserService` implements the same trait or is the same type, it replaces the original in the state struct.
- A simpler V1 could just be: let the user construct `AppState` manually and pass it in, bypassing the builder entirely.

---

## 3. Per-Handler Raw Extractors

### Problem

R2E controllers use `#[inject]` and `#[inject(identity)]` for extractors. But Axum has a rich ecosystem of third-party extractors (e.g. `axum-extra`'s `TypedHeader`, `ConnectInfo`, custom extractors). There's no obvious way to use them inside a controller handler.

### Proposed API

```rust
#[routes]
impl MyController {
    #[get("/info")]
    async fn info(
        &self,
        // R2E-managed injection
        #[inject(identity)] user: AuthenticatedUser,
        // Escape hatch: raw Axum extractor, passed through as-is
        #[raw] ConnectInfo(addr): ConnectInfo<SocketAddr>,
        #[raw] TypedHeader(user_agent): TypedHeader<headers::UserAgent>,
    ) -> Json<Info> {
        Json(Info {
            ip: addr.to_string(),
            agent: user_agent.to_string(),
        })
    }
}
```

### Implementation notes

- `#[raw]` tells the proc macro: "don't try to resolve this via DI, just pass it through as a regular Axum `FromRequestParts` / `FromRequest` extractor."
- This is low-cost to implement — the macro just needs to skip the DI resolution for that parameter and let Axum handle it.
- It might already work if controller parameters without `#[inject]` are passed through to Axum. If so, just document it clearly.

---

## 4. Interceptor with Access to DI Context

### Problem

Custom interceptors currently receive an `InterceptorContext` but have no access to injected services. If you want an interceptor that, say, logs to an audit database, you need access to the DB pool.

### Proposed API

```rust
pub struct AuditInterceptor {
    pub pool: PgPool,
}

impl AuditInterceptor {
    // Constructed from state — the framework resolves this
    pub fn from_state(state: &AppState) -> Self {
        Self { pool: state.pool.clone() }
    }
}

impl<R: Send> Interceptor<R> for AuditInterceptor {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move {
            let result = next().await;
            sqlx::query("INSERT INTO audit_log (method) VALUES ($1)")
                .bind(ctx.method_name)
                .execute(&self.pool)
                .await
                .ok();
            result
        }
    }
}

// Usage
#[post("/")]
#[intercept(AuditInterceptor::from_state)]  // resolved at request time from state
async fn create(&self, body: Json<Request>) -> Json<Response> { /* ... */ }
```

### Implementation notes

- This requires the interceptor instantiation to be deferred to request time (or at least to app startup with access to state).
- A simpler alternative: let interceptors be defined as fields on the controller with `#[inject]`, so they participate in DI like everything else.

---

## 5. Opt-Out of Controller for Individual Modules

### Problem

Sometimes part of your app doesn't fit the controller model at all — a gRPC service, a background worker pool, a custom TCP listener. You still want it to participate in the DI and config system.

### Proposed API

```rust
#[derive(ServiceComponent)]
#[component(state = AppState)]
pub struct MetricsAggregator {
    #[inject] db: DatabasePool,
    #[config("metrics.interval")] interval_secs: u64,
}

impl MetricsAggregator {
    /// Called once at startup — not an HTTP handler
    pub async fn start(self) {
        loop {
            self.aggregate().await;
            tokio::time::sleep(Duration::from_secs(self.interval_secs)).await;
        }
    }
}

// Registration
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .spawn_component::<MetricsAggregator>()  // runs in background, not an HTTP controller
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Implementation notes

- This separates "DI + config resolution" from "HTTP routing." The macro resolves `#[inject]` and `#[config]` the same way, but instead of generating Axum handlers, it just constructs the struct and hands it off.
- The `#[scheduled]` system already does something similar — this would generalize it.

---

## Summary

| Escape Hatch | Complexity | Value |
|---|---|---|
| Raw Axum router mounting | Low | High — unblocks any HTTP edge case |
| Programmatic DI / bean override | Medium | High — essential for testing and runtime config |
| `#[raw]` extractor passthrough | Low | Medium — enables Axum ecosystem interop |
| Interceptors with DI access | Medium | Medium — needed for real-world AOP |
| Non-HTTP components with DI | Medium | Medium — generalizes the DI system |

The principle behind all of these: **R2E should be a layer you can peel back, not a box you're locked into.**