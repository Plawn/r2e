# CLI Scaffolding

The `r2e` CLI generates project scaffolds and code templates, saving you from writing boilerplate.

## Creating a new project

```bash
r2e new my-app
```

In interactive mode (the default when no flags are provided), you'll be prompted to choose:
- **Database**: None, SQLite, PostgreSQL, or MySQL
- **Features**: Authentication, OpenAPI, Task Scheduling, Event Bus, gRPC Server

Or use flags directly:

```bash
# Full-featured project with SQLite
r2e new my-app --full

# PostgreSQL with auth and OpenAPI
r2e new my-app --db postgres --auth --openapi

# gRPC server with scheduler
r2e new my-app --grpc

# Minimal project, no interactive prompts
r2e new my-app --no-interactive
```

### Generated structure

A minimal project:

```
my-app/
  .gitignore
  Cargo.toml
  application.yaml
  src/
    main.rs
    controllers/
      mod.rs
      hello.rs
```

With `--db sqlite`, you also get a `migrations/` directory and a `SqlitePool`
provided as a bean in `main.rs`. With `--auth`, the builder provides an
`Arc<JwtClaimsValidator>` bean. With `--openapi`, the builder includes
`OpenApiPlugin` and your API docs are served at `/docs`. With `--grpc`, you get a
`proto/greeter.proto` sample, a `build.rs`, and `GrpcServer` plugin in the
builder.

### Generated `main.rs`

The generated `main.rs` uses `AppBuilder` with all selected features wired in:

```rust
use r2e::prelude::*;
use r2e::plugins::{Health, Tracing};

mod controllers;

use controllers::hello::HelloController;

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    AppBuilder::new()
        .build_state()
        .await
        .with(Health)
        .with(Tracing)
        .register_controller::<HelloController>()
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
```

With `--full`, additional plugins are added automatically: `Scheduler`, `GrpcServer`, `OpenApiPlugin`, and `SqlitePool`, `LocalEventBus`, and `JwtClaimsValidator` are provided as beans.

### Generated `application.yaml`

```yaml
app:
  name: "my-app"
  port: 8080
```

With `--db sqlite`:
```yaml
database:
  url: "sqlite:data.db?mode=rwc"
```

With `--auth`:
```yaml
security:
  jwt:
    issuer: "my-app"
    audience: "my-app"
    jwks-url: "${JWKS_URL}"
```

With `--grpc`:
```yaml
grpc:
  port: 50051
```

---

## Code generation

### Controllers

```bash
r2e generate controller UserController
```

Generates `src/controllers/user_controller.rs` with a skeleton controller:

```rust
#[controller]
pub struct UserController {
    // #[inject]
    // your_service: YourService,
}

#[routes]
impl UserController {
    #[get("/your-path")]
    async fn list(&self) -> Json<String> {
        Json("Hello from UserController".into())
    }
}
```

Updates `src/controllers/mod.rs` with `pub mod user_controller;`.

### Services

```bash
r2e generate service UserService
```

Generates `src/user_service.rs` with a skeleton service struct:

```rust
#[derive(Clone)]
pub struct UserService {
    // Add your dependencies here
}

impl UserService {
    pub fn new() -> Self {
        Self {}
    }
}
```

### CRUD scaffolding

```bash
r2e generate crud Article --fields "title:String body:String published:bool"
```

Generates a complete CRUD set — **5 files in one command**:

| File | What it contains |
|------|------------------|
| `src/models/article.rs` | `Article` entity + `CreateArticleRequest` / `UpdateArticleRequest` |
| `src/services/article_service.rs` | Service with `#[bean]`, list/get/create/update/delete methods |
| `src/controllers/article_controller.rs` | REST controller at `/articles` with GET, POST, PUT, DELETE |
| `migrations/<timestamp>_create_articles.sql` | SQL migration (only if `migrations/` directory exists) |
| `tests/article_test.rs` | Integration test skeleton |

**Field format:** `name:Type` pairs. Supported types:

| Rust Type | SQL Column | Example |
|-----------|-----------|---------|
| `String` | `TEXT NOT NULL` | `title:String` |
| `i64` | `INTEGER NOT NULL` | `age:i64` |
| `f64` | `REAL NOT NULL` | `price:f64` |
| `bool` | `BOOLEAN NOT NULL` | `active:bool` |
| `Option<String>` | `TEXT` (nullable) | `bio:Option<String>` |
| `Option<i64>` | `INTEGER` (nullable) | `parent_id:Option<i64>` |

An `id INTEGER PRIMARY KEY AUTOINCREMENT` column is always generated automatically.

**After generation, you need to:**

1. Register the controller: `.register_controller::<ArticleController>()`
2. Register the service as a bean: `.register::<ArticleService>()` (before `build_state()`)
3. Run the SQL migration
4. Run `cargo check`

### Middleware (interceptors)

```bash
r2e generate middleware AuditLog
```

Generates `src/middleware/audit_log.rs` with a self-contained `Interceptor<R>`
implementation (the `SelfBuilt` opt-in makes it usable in `#[intercept(...)]`):

```rust
pub struct AuditLog;

impl SelfBuilt for AuditLog {}

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            tracing::info!(method = method_name, "AuditLog: before");
            let result = next().await;
            tracing::info!(method = method_name, "AuditLog: after");
            result
        }
    }
}
```

Use it on controller methods with `#[intercept(AuditLog)]`.

### gRPC services

```bash
r2e generate grpc-service User --package myapp
```

Generates two files:

**`proto/user.proto`** — service definition with `GetUser` and `ListUser` RPCs:

```protobuf
syntax = "proto3";

package myapp;

service User {
  rpc GetUser (GetUserRequest) returns (GetUserResponse);
  rpc ListUser (ListUserRequest) returns (ListUserResponse);
}
```

**`src/grpc/user.rs`** — Rust implementation with `#[grpc_routes]`:

```rust
#[controller]
pub struct UserService { ... }

#[grpc_routes(proto::user_server::User)]
impl UserService {
    async fn get_user(&self, request: tonic::Request<GetUserRequest>) -> Result<tonic::Response<GetUserResponse>, tonic::Status> {
        // ...
    }

    async fn list_user(&self, _request: tonic::Request<ListUserRequest>) -> Result<tonic::Response<ListUserResponse>, tonic::Status> {
        // ...
    }
}
```

**After generation:**

1. Add to `build.rs`: `tonic_build::compile_protos("proto/user.proto")?;`
2. Register: `.register_grpc_service::<UserService>()`
3. `cargo build` to generate proto code

---

## Development server

```bash
r2e dev
```

Wraps `cargo watch` with R2E defaults:
- Watches `src/`, `application.yaml`, `migrations/`
- Prints discovered routes before starting

Use `--open` to auto-open the browser:

```bash
r2e dev --open
```

Requires `cargo-watch`: `cargo install cargo-watch`.

---

## Project health check

```bash
r2e doctor
```

Runs 8 diagnostics against the current directory:

```
R2E Doctor — Checking project health

  ✓ Cargo.toml exists — Found
  ✓ R2E dependency — Found
  ✓ Configuration file — application.yaml found
  ✓ Controllers directory — 3 controller files
  ✓ Rust toolchain — rustc 1.82.0
  ! cargo-watch (for r2e dev) — Not installed. Run: cargo install cargo-watch
  ✓ Migrations directory — 5 migration files
  ✓ Application entrypoint — serve() call found in main.rs

1 issue(s) found
```

Checks include: Cargo.toml existence, R2E dependency, configuration file, controllers directory, Rust toolchain, cargo-watch, migrations directory (if data features used), and `.serve()` call in main.rs.

---

## Route listing

```bash
r2e routes
```

Displays all routes parsed from source code (no compilation needed):

```
Declared routes:

  METHOD   PATH                                HANDLER                   FILE
  --------------------------------------------------------------------------------
  GET      /                                   hello                     hello.rs:5
  GET      /users                              list                      user_controller.rs:12
  POST     /users                              create [admin]            user_controller.rs:22
  DELETE   /users/{id}                         delete [admin]            user_controller.rs:32

  4 routes total
```

Methods are color-coded: GET (green), POST (blue), PUT (yellow), DELETE (red), PATCH (magenta). Role annotations from `#[roles("...")]` appear in brackets.

---

## Extension management

```bash
r2e add security    # adds r2e-security to Cargo.toml
r2e add data-sqlx   # adds the managed SQLx transaction backend
r2e add openapi     # adds r2e-openapi
r2e add events      # adds r2e-events
r2e add scheduler   # adds r2e-scheduler
r2e add cache       # adds r2e-cache
r2e add rate-limit  # adds r2e-rate-limit
r2e add utils       # adds r2e-utils (Logged, Timed, Cache interceptors)
r2e add grpc        # adds r2e-grpc
r2e add test        # adds r2e-test (TestApp, TestJwt)
```

If the dependency is already present, prints a warning without duplicating it.
