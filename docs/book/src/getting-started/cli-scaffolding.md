# CLI Scaffolding

The `r2e` CLI generates project scaffolds and code templates, saving you from writing boilerplate.

## Creating a new project

```bash
r2e new my-app
```

In interactive mode, you'll be prompted to choose:
- **Database**: None, SQLite, PostgreSQL, or MySQL
- **Features**: Authentication, OpenAPI, Events, Scheduling

Or use flags directly:

```bash
# Full-featured project with SQLite
r2e new my-app --full

# PostgreSQL with auth and OpenAPI
r2e new my-app --db postgres --auth --openapi

# Minimal project with no interactive prompts
r2e new my-app --no-interactive
```

### Generated structure

```
my-app/
  Cargo.toml
  application.yaml
  src/
    main.rs
    state.rs
    controllers/
      mod.rs
      hello.rs
```

With `--db`, you also get a `migrations/` directory. With `--auth`, the state includes a `JwtClaimsValidator`. With `--openapi`, the builder includes `OpenApiPlugin`.

## Code generation

### Controllers

```bash
r2e generate controller UserController
```

Generates `src/controllers/user_controller.rs` with a skeleton controller and updates `src/controllers/mod.rs`.

### Services

```bash
r2e generate service UserService
```

Generates `src/user_service.rs` with a skeleton service struct.

### CRUD scaffolding

```bash
r2e generate crud Article --fields "title:String body:String published:bool"
```

Generates a complete CRUD set:
- `src/models/article.rs` — entity struct + `CreateArticle`/`UpdateArticle` request types
- `src/services/article_service.rs` — service with list/get/create/update/delete
- `src/controllers/article_controller.rs` — REST controller (GET, POST, PUT, DELETE)
- `migrations/<timestamp>_create_articles.sql` — SQL migration (if `migrations/` exists)
- `tests/article_test.rs` — integration test skeleton

### Middleware (interceptors)

```bash
r2e generate middleware AuditLog
```

Generates `src/middleware/audit_log.rs` with an `Interceptor<R>` implementation skeleton.

## Development server

```bash
r2e dev
```

Wraps `cargo watch` with R2E defaults:
- Watches `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`
- Sets `R2E_PROFILE=dev`
- Prints discovered routes before starting

Use `--open` to auto-open the browser:

```bash
r2e dev --open
```

Requires `cargo-watch`: `cargo install cargo-watch`.

## Project health check

```bash
r2e doctor
```

Runs diagnostics:
- Cargo.toml exists and has R2E dependency
- Configuration file present
- Controllers directory exists
- Rust toolchain available
- `cargo-watch` installed (for `r2e dev`)
- Migrations directory (if using data features)
- Application entrypoint has `.serve()` call

## Route listing

```bash
r2e routes
```

Displays all routes from your controllers (parsed from source, no compilation needed):

```
GET     /health
GET     /users
GET     /users/{id}
POST    /users               [roles: admin]
DELETE  /users/{id}          [roles: admin]
```

## Extension management

```bash
r2e add security    # adds r2e-security
r2e add data        # adds r2e-data + r2e-data-sqlx
r2e add openapi     # adds r2e-openapi
r2e add events      # adds r2e-events
r2e add scheduler   # adds r2e-scheduler
r2e add cache       # adds r2e-cache
r2e add rate-limit  # adds r2e-rate-limit
r2e add utils       # adds r2e-utils
r2e add test        # adds r2e-test (dev-dependency)
```
