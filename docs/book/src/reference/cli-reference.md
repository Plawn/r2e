# CLI Reference

The `r2e` CLI provides project scaffolding, code generation, development tools, and diagnostics.

## Installation

```bash
cargo install r2e-cli
```

This installs the `r2e` binary. Verify with `r2e --version`.

---

## `r2e new <name>`

Create a new R2E project.

```
r2e new <name> [options]

Options:
  --db <sqlite|postgres|mysql>   Include database support
  --auth                         Include JWT/OIDC security
  --openapi                      Include OpenAPI documentation
  --metrics                      Include Prometheus metrics (reserved)
  --grpc                         Include gRPC server support
  --full                         Enable all features (SQLite + auth + openapi + scheduler + events + gRPC)
  --no-interactive               Skip interactive prompts, use flags/defaults
```

**Mode selection:**

| Condition | Behavior |
|-----------|----------|
| `--full` | All features enabled (SQLite, auth, openapi, scheduler, events, gRPC) |
| `--no-interactive` or any flag set | Use provided flags, defaults for the rest |
| No flags | Interactive prompts (database choice, feature multi-select) |

**Database aliases:** `--db postgres` and `--db pg` both select PostgreSQL.

**Examples:**

```bash
r2e new my-app                              # Interactive mode
r2e new my-app --full                       # Full-featured, SQLite
r2e new my-app --db postgres --auth         # PostgreSQL + JWT
r2e new my-app --openapi --grpc             # OpenAPI docs + gRPC server
r2e new my-app --no-interactive             # Minimal, no prompts
```

### Generated project structure

**Minimal project** (no feature flags):

```
my-app/
  .gitignore                      /target
  Cargo.toml                      r2e + tokio + serde + tracing
  application.yaml                app name + port
  src/
    main.rs                       #[tokio::main], AppBuilder, .serve()
    state.rs                      AppState with #[derive(Clone, BeanState)]
    controllers/
      mod.rs                      pub mod hello;
      hello.rs                    HelloController at /
```

**Additional files by feature:**

| Feature | Generated files / changes |
|---------|--------------------------|
| `--db sqlite` | `migrations/` dir, `sqlx` dep (sqlite), `SqlitePool` in state |
| `--db postgres` | `migrations/` dir, `sqlx` dep (postgres), `PgPool` in state |
| `--db mysql` | `migrations/` dir, `sqlx` dep (mysql), `MySqlPool` in state |
| `--auth` | `r2e` security feature, `Arc<JwtClaimsValidator>` in state, JWT config in YAML |
| `--openapi` | `r2e` openapi feature, `OpenApiPlugin` in builder, `/docs` UI |
| `--grpc` | `tonic` + `prost` deps, `build.rs`, `proto/greeter.proto`, `GrpcServer` plugin |
| `--full` | All of the above (SQLite + auth + openapi + scheduler + events + gRPC) |

### Generated `main.rs` (with `--full`)

```rust
use r2e::prelude::*;
use r2e::plugins::{Health, Tracing};
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

mod controllers;
mod state;

use controllers::hello::HelloController;
use state::AppState;

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    AppBuilder::new()
        .plugin(Scheduler)
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .build_state::<AppState, _>()
        .await
        .with(Health)
        .with(Tracing)
        .with(OpenApiPlugin::new(OpenApiConfig::new("API", "0.1.0").with_docs_ui(true)))
        .register_controller::<HelloController>()
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
```

### Generated `application.yaml` (with `--full`)

```yaml
app:
  name: "my-app"
  port: 8080

database:
  url: "sqlite:data.db?mode=rwc"

security:
  jwt:
    issuer: "my-app"
    audience: "my-app"
    jwks-url: "${JWKS_URL}"

grpc:
  port: 50051
```

---

## `r2e generate`

Generate code scaffolds. All names are converted from PascalCase to snake_case for file names.

### `r2e generate controller <Name>`

```bash
r2e generate controller UserController
```

**Creates:** `src/controllers/user_controller.rs`

```rust
use axum::Json;
use r2e_core::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Controller)]
#[controller(state = AppState)]
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

**Updates:** `src/controllers/mod.rs` — appends `pub mod user_controller;` (if mod.rs exists).

**Error:** if the file already exists.

### `r2e generate service <Name>`

```bash
r2e generate service UserService
```

**Creates:** `src/user_service.rs`

```rust
use std::sync::Arc;

#[derive(Clone)]
pub struct UserService {
    // Add your dependencies here
}

impl UserService {
    pub fn new() -> Self {
        Self {}
    }

    // Add your methods here
}
```

**Error:** if the file already exists.

### `r2e generate crud <Name> --fields "<fields>"`

Generate a complete CRUD set: model, service, controller, migration, and test skeleton.

```bash
r2e generate crud Article --fields "title:String body:String published:bool"
```

**Creates 5 files:**

| File | Content |
|------|---------|
| `src/models/article.rs` | `Article` entity + `CreateArticleRequest` / `UpdateArticleRequest` |
| `src/services/article_service.rs` | Service with `#[bean]`, CRUD methods using `sqlx::query_as!` |
| `src/controllers/article_controller.rs` | REST controller at `/articles` with GET, POST, PUT, DELETE |
| `migrations/<timestamp>_create_articles.sql` | `CREATE TABLE` with typed columns (only if `migrations/` exists) |
| `tests/article_test.rs` | Integration test skeleton with `TestApp` + `TestJwt` |

Each parent directory gets a `mod.rs` created or updated with the appropriate `pub mod` declaration.

#### Field format

Fields are `name:Type` pairs. Multiple fields are passed as separate args after `--fields`:

```bash
r2e generate crud User --fields "name:String email:String age:i64 active:bool"
```

**Type mapping:**

| Rust Type | SQL Type | Nullable |
|-----------|----------|----------|
| `String`, `&str` | `TEXT` | `NOT NULL` |
| `i32`, `i64`, `u32`, `u64`, `usize` | `INTEGER` | `NOT NULL` |
| `f32`, `f64` | `REAL` | `NOT NULL` |
| `bool` | `BOOLEAN` | `NOT NULL` |
| `Option<String>` | `TEXT` | (nullable) |
| `Option<i64>` | `INTEGER` | (nullable) |
| Unknown type | `TEXT` | `NOT NULL` |

`Option<T>` fields are nullable in the migration (no `NOT NULL` constraint) and `is_optional = true` in the model.

An `id: i64` primary key column is always generated automatically — do not include it in `--fields`.

#### Generated migration example

For `r2e generate crud User --fields "name:String email:Option<String> age:i64"`:

```sql
-- Create users table
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT,
    age INTEGER NOT NULL
);
```

#### Generated controller example

```rust
#[derive(Controller)]
#[controller(path = "/articles", state = AppState)]
pub struct ArticleController {
    #[inject]
    service: ArticleService,
}

#[routes]
impl ArticleController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<Article>> { ... }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<Article>, AppError> { ... }

    #[post("/")]
    async fn create(&self, Json(body): Json<CreateArticleRequest>) -> Json<Article> { ... }

    #[put("/{id}")]
    async fn update(&self, Path(id): Path<i64>, Json(body): Json<UpdateArticleRequest>) -> Result<Json<Article>, AppError> { ... }

    #[delete("/{id}")]
    async fn delete(&self, Path(id): Path<i64>) -> Result<Json<&'static str>, AppError> { ... }
}
```

#### Next steps after CRUD generation

1. Register the controller in `main.rs`: `.register_controller::<ArticleController>()`
2. Add `ArticleService` to your state struct (or use `#[bean]` DI)
3. Run migrations if applicable
4. Run `cargo check` to verify

### `r2e generate middleware <Name>`

```bash
r2e generate middleware AuditLog
```

**Creates:** `src/middleware/audit_log.rs`

```rust
use r2e::prelude::*;
use std::future::Future;

/// Custom interceptor: AuditLog
pub struct AuditLog;

impl<R: Send, S: Send + Sync> Interceptor<R, S> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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

**Updates:** `src/middleware/mod.rs` — appends `pub mod audit_log;`.

Use it with `#[intercept(AuditLog)]` on controller methods.

### `r2e generate grpc-service <Name> [--package <pkg>]`

```bash
r2e generate grpc-service User
r2e generate grpc-service User --package myapp
```

**Creates:**

| File | Content |
|------|---------|
| `proto/user.proto` | Service with `GetUser` and `ListUser` RPCs, request/response messages |
| `src/grpc/user.rs` | Controller with `#[grpc_routes]` and placeholder RPC implementations |

The `--package` flag sets the protobuf package name (default: `myapp`).

**Generated proto:**

```protobuf
syntax = "proto3";

package myapp;

service User {
  rpc GetUser (GetUserRequest) returns (GetUserResponse);
  rpc ListUser (ListUserRequest) returns (ListUserResponse);
}

message GetUserRequest {
  string id = 1;
}

message GetUserResponse {
  string id = 1;
  string name = 2;
}

message ListUserRequest {
  int32 page_size = 1;
  string page_token = 2;
}

message ListUserResponse {
  repeated GetUserResponse items = 1;
  string next_page_token = 2;
}
```

**Next steps after gRPC generation:**

1. Add to `build.rs`: `tonic_build::compile_protos("proto/user.proto")?;`
2. Register in `main.rs`: `.register_grpc_service::<UserService>()`
3. Run `cargo build` to generate proto code

---

## `r2e dev`

Start development server with hot-reload.

```
r2e dev [options]

Options:
  --open    Open browser at http://localhost:8080 after a 5-second delay
```

**Behavior:**

1. Checks that `cargo-watch` is installed (errors if not)
2. Prints discovered routes via `r2e routes`
3. Spawns `cargo watch` with:
   - Watched paths: `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`
   - Ignored: `target/`
   - Command: `cargo run`
4. Sets `R2E_PROFILE=dev` environment variable
5. Waits for the process (Ctrl+C to stop)

**Prerequisite:**

```bash
cargo install cargo-watch
```

---

## `r2e doctor`

Run 8 project health checks against the current directory.

```bash
r2e doctor
```

**Checks:**

| # | Check | Level | Condition |
|---|-------|-------|-----------|
| 1 | Cargo.toml exists | Error | `Cargo.toml` present in CWD |
| 2 | R2E dependency | Error | `Cargo.toml` contains `r2e` |
| 3 | Configuration file | Warning | `application.yaml` exists |
| 4 | Controllers directory | Warning | `src/controllers/` exists (counts `.rs` files) |
| 5 | Rust toolchain | Error | `rustc --version` succeeds |
| 6 | cargo-watch | Warning | `cargo watch --version` succeeds |
| 7 | Migrations directory | Warning | If `r2e-data` or `"data"` in Cargo.toml, checks `migrations/` |
| 8 | Application entrypoint | Warning | `src/main.rs` contains `.serve(` |

**Output indicators:**
- `✓` (green) — check passed
- `!` (yellow) — warning (non-blocking issue)
- `x` (red) — error (critical issue)

**Example:**

```
R2E Doctor — Checking project health

  ✓ Cargo.toml exists — Found
  ✓ R2E dependency — Found
  ✓ Configuration file — application.yaml found
  ✓ Controllers directory — 3 controller files
  ✓ Rust toolchain — rustc 1.82.0 (f6e511eec 2024-10-15)
  ! cargo-watch (for r2e dev) — Not installed. Run: cargo install cargo-watch
  ✓ Migrations directory — 5 migration files
  ✓ Application entrypoint — serve() call found in main.rs

1 issue(s) found
```

---

## `r2e routes`

List all declared routes by parsing source files (no compilation).

```bash
r2e routes
```

Scans `src/controllers/*.rs` (excluding `mod.rs`) and extracts:

- `#[controller(path = "...")]` — base path
- `#[get("/...")]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` — HTTP methods + paths
- `#[roles("...")]` — role annotations
- Next `fn` after the attribute — handler name

Routes are sorted by path. Methods are color-coded: GET (green), POST (blue), PUT (yellow), DELETE (red), PATCH (magenta).

**Example:**

```
Declared routes:

  METHOD   PATH                                HANDLER                   FILE
  --------------------------------------------------------------------------------
  GET      /                                   hello                     hello.rs:5
  GET      /users                              list                      user_controller.rs:12
  GET      /users/{id}                         get_by_id                 user_controller.rs:17
  POST     /users                              create                    user_controller.rs:22
  DELETE   /users/{id}                         delete [admin]            user_controller.rs:32

  5 routes total
```

**Error:** if `src/controllers/` directory does not exist.

---

## `r2e add <extension>`

Add an R2E sub-crate dependency to `Cargo.toml`.

```bash
r2e add <extension>
```

Parses `Cargo.toml` using `toml_edit`, adds the crate with version `0.1` to `[dependencies]`. Errors if the extension is unknown or `Cargo.toml` is missing. Prints a warning (no error) if already present.

**Available extensions:**

| Extension | Crate | Description |
|-----------|-------|-------------|
| `security` | `r2e-security` | JWT/OIDC authentication, `AuthenticatedUser`, role extraction |
| `data` | `r2e-data` | Entity, Repository, QueryBuilder abstractions |
| `data-sqlx` | `r2e-data-sqlx` | SQLx backend for Repository |
| `data-diesel` | `r2e-data-diesel` | Diesel backend for Repository |
| `openapi` | `r2e-openapi` | OpenAPI 3.0.3 spec generation + Swagger UI at `/docs` |
| `events` | `r2e-events` | In-process typed event bus (emit, subscribe) |
| `scheduler` | `r2e-scheduler` | Background task scheduling (cron, interval, delay) |
| `cache` | `r2e-cache` | TTL cache with pluggable backends |
| `rate-limit` | `r2e-rate-limit` | Token-bucket rate limiting (global, per-IP, per-user) |
| `utils` | `r2e-utils` | Built-in interceptors: Logged, Timed, Cache, CacheInvalidate |
| `prometheus` | `r2e-prometheus` | Prometheus metrics |
| `grpc` | `r2e-grpc` | gRPC server support (tonic-based) |
| `test` | `r2e-test` | Test helpers: TestApp (HTTP client), TestJwt (JWT gen) |

**Example:**

```bash
r2e add security      # adds r2e-security = "0.1"
r2e add data          # adds r2e-data = "0.1"
r2e add test          # adds r2e-test = "0.1"
```

---

## Name conversion rules

The CLI automatically converts between naming conventions:

| Function | Input | Output |
|----------|-------|--------|
| `to_snake_case` | `UserController` | `user_controller` |
| `to_snake_case` | `BlogPost` | `blog_post` |
| `to_snake_case` | `HTTPClient` | `h_t_t_p_client` |
| `to_pascal_case` | `user_service` | `UserService` |
| `to_pascal_case` | `my_cool_service` | `MyCoolService` |
| `pluralize` | `user` | `users` |
| `pluralize` | `category` | `categories` |
| `pluralize` | `status` | `statuses` |
| `pluralize` | `crash` | `crashes` |
