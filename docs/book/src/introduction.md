# Introduction

**R2E** (Rust Enterprise Edition) is an ergonomic enterprise framework for Rust, inspired by Quarkus. It provides declarative controllers, compile-time dependency injection, JWT/OIDC security, and zero runtime reflection. Built on [Axum](https://github.com/tokio-rs/axum) via the `r2e-http` abstraction layer.

## What R2E offers

```rust
#[controller(path = "/users")]
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

- **Declarative controllers** — `#[controller]` + `#[routes]` generate Axum handlers with zero boilerplate
- **Compile-time DI** — `#[inject]` for services, `#[inject(identity)]` for request-scoped identity, `#[config("key")]` for configuration
- **JWT/OIDC security** — `AuthenticatedUser` extractor with JWKS caching, role-based access via `#[roles("admin")]`
- **Guards** — Pre-auth and post-auth guards for custom authorization logic
- **Interceptors** — AOP-style `#[intercept(...)]` for logging, timing, caching, and custom cross-cutting concerns
- **Rate limiting** — Token-bucket rate limiting per user, per IP, or global
- **Event bus** — Typed in-process pub/sub with `#[consumer]` for declarative event handlers
- **Scheduling** — `#[scheduled(every = 30)]` and `#[scheduled(cron = "...")]` for background tasks
- **Managed resources** — `#[managed]` for automatic transaction lifecycle
- **Database ergonomics** — core pagination and managed SQLx/Diesel transactions
- **Validation** — Automatic validation via `garde` crate — just derive `Validate` and use `Json<T>`
- **OpenAPI** — Auto-generated OpenAPI 3.1.0 spec with interactive docs UI
- **Configuration** — YAML + env var overlay with profile support
- **Static file serving** — Embed frontend assets in the binary with SPA fallback via `EmbeddedFrontend` plugin
- **SSE & WebSocket** — Built-in `SseBroadcaster` and `WsRooms` for real-time communication
- **Testing** — `TestApp` HTTP client wrapper, `TestJwt`, and `r2e test --sonarqube` coverage
- **MCP server** — expose beans to LLM agents as Model Context Protocol tools, resources and prompts, with the same guards and identity as HTTP
- **gRPC** — Tonic services on a separate port or multiplexed with HTTP, sharing JWT validation and guards
- **Multi-tenancy** — per-tenant bean routing with `Tenanted<T>`, tenant resolvers and per-tenant database pools
- **OpenFGA authorization** — schema-first `model!` macro and typed relationship-check guards
- **QUIC / HTTP/3** — serve HTTP/3 next to HTTP/1.1 and HTTP/2, with `Alt-Svc` advertising
- **Prometheus and observability** — HTTP metrics with route-template labels, OpenTelemetry tracing over OTLP
- **Graceful lifecycle** — bounded HTTP drain, `on_drain`/`on_stop` hooks and programmatic `StopHandle`
- **Sharded serving** — optional thread-per-core workers with `SO_REUSEPORT` listeners
- **Background services** — managed task pool, `#[async_exec]` and `#[derive(BackgroundService)]` workers drained on shutdown
- **Live configuration** — `LiveConfig<T>` values updated at runtime by config providers such as Vault
- **CLI** — `r2e new`, `r2e add`, `r2e dev`, `r2e generate`, `r2e test` for scaffolding and workflows

## How this book is organized

- **Getting Started** — Install R2E, create your first app, learn the project structure
- **Core Concepts** — Controllers, DI, beans, plugins, configuration, live configuration, error handling, validation
- **Security** — JWT authentication, embedded OIDC, guards, roles, rate limiting, multi-tenancy, OpenFGA
- **Data Access** — Pagination, managed transactions
- **Events and Scheduling** — Event bus, consumers, scheduling, dynamic tasks, executor and background services
- **Real-Time** — Server-Sent Events, WebSocket
- **gRPC** — gRPC services, guards and identity
- **MCP** — MCP server: tools, resources, prompts, auth
- **Advanced** — Interceptors, custom guards/plugins, managed resources, lifecycle hooks, catch-all routes, OpenAPI, metrics, caching, serve lifecycle, sharded serving, QUIC/HTTP3, performance
- **Testing** — TestApp, TestJwt, integration patterns
- **Reference** — Crate map, CLI reference, API docs, migration guides
