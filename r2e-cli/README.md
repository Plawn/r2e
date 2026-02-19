# R2E CLI

Command-line tool for scaffolding and managing [R2E](https://github.com/anthropics/r2e) projects. Provides project creation, code generation, diagnostics, route listing, and a development server with hot-reload.

## Installation

```bash
cargo install --path r2e-cli
```

This installs the `r2e` binary globally.

## Commands

| Command | Description |
|---------|-------------|
| [`r2e new`](#r2e-new-name) | Create a new R2E project |
| [`r2e generate`](#r2e-generate) | Generate controllers, services, CRUD, middleware, gRPC |
| [`r2e add`](#r2e-add-extension) | Add an extension to your project |
| [`r2e dev`](#r2e-dev) | Start development server with hot-reload |
| [`r2e doctor`](#r2e-doctor) | Check project health |
| [`r2e routes`](#r2e-routes) | List all declared routes |

---

### `r2e new <name>`

Create a new R2E project with an optional feature selection.

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

**Interactive mode** (default): when no flags are provided, prompts you for database and feature selection using `dialoguer`.

**Examples:**

```bash
r2e new my-app                              # Interactive — prompts for choices
r2e new my-app --full                       # All features, SQLite
r2e new my-app --db postgres --auth         # PostgreSQL + JWT auth
r2e new my-app --openapi --grpc             # OpenAPI + gRPC
r2e new my-app --no-interactive             # Minimal, no prompts
```

**Generated structure** (with `--full`):

```
my-app/
  .gitignore
  Cargo.toml                    # r2e + tokio + serde + sqlx + tonic + prost
  application.yaml              # App config (database, security, gRPC)
  build.rs                      # tonic-build for proto compilation
  migrations/                   # SQLx migration directory
  proto/
    greeter.proto               # Sample gRPC service definition
  src/
    main.rs                     # #[tokio::main] with AppBuilder + .serve()
    state.rs                    # AppState with BeanState derive
    controllers/
      mod.rs
      hello.rs                  # Hello world controller
```

**Minimal project** (no flags):

```
my-app/
  .gitignore
  Cargo.toml
  application.yaml
  src/
    main.rs
    state.rs
    controllers/
      mod.rs
      hello.rs
```

---

### `r2e generate`

Generate code scaffolds. Names are automatically converted from PascalCase to snake_case for file names.

#### `r2e generate controller <Name>`

```bash
r2e generate controller UserController
```

Creates `src/controllers/user_controller.rs` with:
- `#[derive(Controller)]` struct with `#[controller(state = AppState)]`
- `#[routes]` impl block with a placeholder `#[get]` endpoint

Automatically appends `pub mod user_controller;` to `src/controllers/mod.rs` (if it exists).

#### `r2e generate service <Name>`

```bash
r2e generate service UserService
```

Creates `src/user_service.rs` with a `#[derive(Clone)]` struct and a `new()` constructor.

#### `r2e generate crud <Name> --fields "<fields>"`

Generate a complete CRUD set: model, service, controller, migration, and test skeleton.

```bash
r2e generate crud Article --fields "title:String body:String published:bool"
```

**Generated files:**

| File | Content |
|------|---------|
| `src/models/article.rs` | `Article` entity + `CreateArticleRequest` / `UpdateArticleRequest` |
| `src/services/article_service.rs` | Service with `#[bean]`, list/get/create/update/delete methods |
| `src/controllers/article_controller.rs` | REST controller: GET, POST, PUT, DELETE on `/articles` |
| `migrations/<timestamp>_create_articles.sql` | `CREATE TABLE` with correct column types (only if `migrations/` exists) |
| `tests/article_test.rs` | Integration test skeleton with `TestApp` + `TestJwt` |

Each directory gets its `mod.rs` updated automatically.

**Field format:** `name:Type` pairs separated by spaces.

| Rust Type | SQL Type | Example |
|-----------|----------|---------|
| `String`, `&str` | `TEXT` | `name:String` |
| `i32`, `i64`, `u32`, `u64`, `usize` | `INTEGER` | `age:i64` |
| `f32`, `f64` | `REAL` | `price:f64` |
| `bool` | `BOOLEAN` | `published:bool` |
| `Option<T>` | `<T> (nullable)` | `bio:Option<String>` |
| Other | `TEXT` (default) | `data:CustomType` |

Optional fields (`Option<T>`) produce nullable columns (no `NOT NULL` constraint).

#### `r2e generate middleware <Name>`

```bash
r2e generate middleware AuditLog
```

Creates `src/middleware/audit_log.rs` with an `Interceptor<R, S>` implementation skeleton (before/after logging with `tracing`). Updates `src/middleware/mod.rs`.

#### `r2e generate grpc-service <Name> [--package <pkg>]`

```bash
r2e generate grpc-service User
r2e generate grpc-service User --package myapp
```

**Generated files:**

| File | Content |
|------|---------|
| `proto/user.proto` | Service definition with `GetUser` and `ListUser` RPCs |
| `src/grpc/user.rs` | Controller with `#[grpc_routes]`, placeholder RPC implementations |

The `--package` flag sets the protobuf package name (default: `myapp`). After generation:

1. Add `tonic_build::compile_protos("proto/user.proto")?;` to `build.rs`
2. Register the service: `.register_grpc_service::<UserService>()`
3. Run `cargo build` to generate proto code

---

### `r2e add <extension>`

Add an R2E sub-crate dependency to your `Cargo.toml`.

```bash
r2e add security
```

If the dependency is already present, prints a warning and does nothing.

**Available extensions:**

| Extension | Crate | Description |
|-----------|-------|-------------|
| `security` | `r2e-security` | JWT/OIDC authentication, `AuthenticatedUser`, role extraction |
| `data` | `r2e-data` | Entity, Repository, QueryBuilder abstractions |
| `data-sqlx` | `r2e-data-sqlx` | SQLx backend for Repository |
| `data-diesel` | `r2e-data-diesel` | Diesel backend for Repository |
| `openapi` | `r2e-openapi` | OpenAPI 3.0.3 spec generation + Swagger UI |
| `events` | `r2e-events` | In-process typed event bus |
| `scheduler` | `r2e-scheduler` | Background task scheduling (cron, interval) |
| `cache` | `r2e-cache` | TTL cache with pluggable backends |
| `rate-limit` | `r2e-rate-limit` | Token-bucket rate limiting |
| `utils` | `r2e-utils` | Built-in interceptors: Logged, Timed, Cache |
| `prometheus` | `r2e-prometheus` | Prometheus metrics |
| `grpc` | `r2e-grpc` | gRPC server support |
| `test` | `r2e-test` | Test helpers: TestApp, TestJwt |

---

### `r2e dev`

Start the development server with hot-reload.

```
r2e dev [options]

Options:
  --open    Open browser at http://localhost:8080 after startup
```

Wraps [`cargo-watch`](https://github.com/watchexec/cargo-watch) with R2E defaults:

- **Watches:** `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`
- **Ignores:** `target/`
- **Environment:** sets `R2E_PROFILE=dev`
- **Routes:** prints discovered routes before starting the watch loop

Requires `cargo-watch`:

```bash
cargo install cargo-watch
```

---

### `r2e doctor`

Run project health diagnostics.

```bash
r2e doctor
```

Runs 8 checks against the current directory:

| Check | Level | What it verifies |
|-------|-------|------------------|
| Cargo.toml exists | Error | Current directory is a Rust project |
| R2E dependency | Error | `r2e` appears in Cargo.toml |
| Configuration file | Warning | `application.yaml` exists |
| Controllers directory | Warning | `src/controllers/` exists (counts `.rs` files) |
| Rust toolchain | Error | `rustc --version` succeeds |
| cargo-watch | Warning | `cargo watch --version` succeeds (needed for `r2e dev`) |
| Migrations directory | Warning | If `r2e-data` in Cargo.toml, `migrations/` exists |
| Application entrypoint | Warning | `src/main.rs` contains a `.serve()` call |

Output uses colored indicators: `✓` (green) for OK, `!` (yellow) for warnings, `x` (red) for errors.

**Example output:**

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

### `r2e routes`

List all declared routes by parsing source files in `src/controllers/` (no compilation needed).

```bash
r2e routes
```

Extracts `#[controller(path = "...")]` base paths, `#[get]` / `#[post]` / `#[put]` / `#[delete]` / `#[patch]` method attributes, handler function names, and `#[roles("...")]` annotations.

**Example output:**

```
Declared routes:

  METHOD   PATH                                HANDLER                   FILE
  --------------------------------------------------------------------------------
  GET      /                                   hello                     hello.rs:5
  GET      /users                              list                      user_controller.rs:12
  GET      /users/{id}                         get_by_id                 user_controller.rs:17
  POST     /users                              create                    user_controller.rs:22
  PUT      /users/{id}                         update                    user_controller.rs:27
  DELETE   /users/{id}                         delete [admin]            user_controller.rs:32

  6 routes total
```

Methods are color-coded: GET (green), POST (blue), PUT (yellow), DELETE (red), PATCH (magenta).

---

## Typical workflow

```bash
# 1. Bootstrap a project
r2e new my-api --db sqlite --auth --openapi
cd my-api

# 2. Generate a full CRUD
r2e generate crud Article --fields "title:String body:String published:bool"

# 3. Add more extensions
r2e add events
r2e add scheduler

# 4. Generate additional components
r2e generate controller DashboardController
r2e generate middleware AuditLog
r2e generate grpc-service Notification

# 5. Check project health
r2e doctor

# 6. See all routes
r2e routes

# 7. Start developing
r2e dev
```

## Name conventions

The CLI converts between naming conventions automatically:

| Input | File name | Struct name |
|-------|-----------|-------------|
| `UserController` | `user_controller.rs` | `UserController` |
| `BlogPost` | `blog_post.rs` | `BlogPost` |
| `HTTPClient` | `h_t_t_p_client.rs` | `HTTPClient` |

Pluralization for CRUD table names: `user` → `users`, `category` → `categories`, `status` → `statuses`.
