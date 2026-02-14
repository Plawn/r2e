# CLI Reference

The `r2e` CLI provides project scaffolding, code generation, development tools, and diagnostics.

## Installation

```bash
cargo install r2e-cli
```

## Commands

### `r2e new <name>`

Create a new R2E project.

```
r2e new <name> [options]

Options:
  --db <sqlite|postgres|mysql>   Include database support
  --auth                         Include JWT/OIDC security
  --openapi                      Include OpenAPI documentation
  --full                         Enable all features (SQLite + all)
  --no-interactive               Skip interactive prompts
```

Examples:
```bash
r2e new my-app                          # Interactive
r2e new my-app --full                   # All features, SQLite
r2e new my-app --db postgres --auth     # Postgres + auth
r2e new my-app --no-interactive         # Minimal, no prompts
```

### `r2e dev`

Start development server with hot-reload.

```
r2e dev [options]

Options:
  --open    Open browser after startup
```

Wraps `cargo watch`. Watches `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`. Sets `R2E_PROFILE=dev`. Requires `cargo install cargo-watch`.

### `r2e generate`

Generate code scaffolds.

#### `r2e generate controller <Name>`

```bash
r2e generate controller UserController
```

Creates `src/controllers/user_controller.rs` with a skeleton controller. Updates `src/controllers/mod.rs`.

#### `r2e generate service <Name>`

```bash
r2e generate service UserService
```

Creates `src/user_service.rs` with a skeleton service struct.

#### `r2e generate crud <Name> --fields "<fields>"`

```bash
r2e generate crud Article --fields "title:String body:String published:bool"
```

Generates:
- `src/models/article.rs` — entity + request types
- `src/services/article_service.rs` — CRUD service
- `src/controllers/article_controller.rs` — REST controller
- `migrations/<timestamp>_create_articles.sql` — SQL migration
- `tests/article_test.rs` — test skeleton

Field format: `name:Type` pairs separated by spaces. Types: `String` (→ TEXT), `i64` (→ INTEGER), `f64` (→ REAL), `bool` (→ BOOLEAN). Prefix with `?` for optional: `?website:String`.

#### `r2e generate middleware <Name>`

```bash
r2e generate middleware AuditLog
```

Creates `src/middleware/audit_log.rs` with an `Interceptor<R>` implementation skeleton.

### `r2e doctor`

Run project health checks.

```bash
r2e doctor
```

Checks:
- Cargo.toml exists and has R2E dependency
- Configuration file present (`application.yaml`)
- Controllers directory exists
- Rust toolchain available
- `cargo-watch` installed
- Migrations directory (if data features used)
- Application entrypoint has `.serve()` call

### `r2e routes`

List all routes from source code (no compilation).

```bash
r2e routes
```

Output:
```
GET     /health
GET     /users
GET     /users/{id}
POST    /users               [roles: admin]
DELETE  /users/{id}          [roles: admin]
```

### `r2e add <extension>`

Add an R2E sub-crate dependency.

```bash
r2e add security      # r2e-security
r2e add data          # r2e-data + r2e-data-sqlx
r2e add openapi       # r2e-openapi
r2e add events        # r2e-events
r2e add scheduler     # r2e-scheduler
r2e add cache         # r2e-cache
r2e add rate-limit    # r2e-rate-limit
r2e add utils         # r2e-utils
r2e add test          # r2e-test (dev-dependency)
```
