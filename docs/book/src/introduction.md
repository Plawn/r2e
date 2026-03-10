# Introduction

**R2E** (Rust Enterprise Edition) is a Quarkus-like ergonomic layer over [Axum](https://github.com/tokio-rs/axum) for Rust. It provides declarative controllers, compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

## What R2E offers

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject]           user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[post("/")]
    #[roles("admin")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }
}
```

If you've used Java's Quarkus, Spring Boot, or C#'s ASP.NET, this should feel familiar — but everything is resolved at compile time with zero runtime reflection.

## Key features

- **Declarative controllers** — `#[derive(Controller)]` + `#[routes]` generate Axum handlers with zero boilerplate
- **Compile-time DI** — `#[inject]` for services, `#[inject(identity)]` for request-scoped identity, `#[config("key")]` for configuration
- **JWT/OIDC security** — `AuthenticatedUser` extractor with JWKS caching, role-based access via `#[roles("admin")]`
- **Guards** — Pre-auth and post-auth guards for custom authorization logic
- **Interceptors** — AOP-style `#[intercept(...)]` for logging, timing, caching, and custom cross-cutting concerns
- **Rate limiting** — Token-bucket rate limiting per user, per IP, or global
- **Event bus** — Typed in-process pub/sub with `#[consumer]` for declarative event handlers
- **Scheduling** — `#[scheduled(every = 30)]` and `#[scheduled(cron = "...")]` for background tasks
- **Managed resources** — `#[managed]` for automatic transaction lifecycle
- **Data access** — `Entity`, `Repository`, `QueryBuilder`, and `Pageable`/`Page`
- **Validation** — Automatic validation via `garde` crate — just derive `Validate` and use `Json<T>`
- **OpenAPI** — Auto-generated OpenAPI 3.0.3 spec with interactive docs UI
- **Configuration** — YAML + env var overlay with profile support
- **Static file serving** — Embed frontend assets in the binary with SPA fallback via `EmbeddedFrontend` plugin
- **SSE & WebSocket** — Built-in `SseBroadcaster` and `WsRooms` for real-time communication
- **Testing** — `TestApp` HTTP client wrapper and `TestJwt` token generator
- **CLI** — `r2e new`, `r2e add`, `r2e dev`, `r2e generate` for scaffolding

## How this book is organized

- **Getting Started** — Install R2E, create your first app, learn the project structure
- **Core Concepts** — Controllers, DI, beans, plugins, configuration, error handling
- **Security** — JWT authentication, guards, roles, rate limiting
- **Data Access** — Entities, repositories, queries, pagination, transactions
- **Events and Scheduling** — Event bus, consumers, background tasks
- **Advanced** — Interceptors, custom guards/plugins, managed resources, lifecycle hooks, OpenAPI, performance
- **Testing** — TestApp, TestJwt, integration patterns
- **Reference** — Crate map, CLI reference, API docs
