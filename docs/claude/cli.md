# CLI (r2e-cli)

The `r2e` binary provides project scaffolding, code generation, diagnostics, and development tooling.

**Key files:**
- `r2e-cli/src/main.rs` — CLI entry point (clap `Commands` + `GenerateKind` enums)
- `r2e-cli/src/commands/` — one module per command
- `r2e-cli/src/commands/templates/` — code generation templates (project, middleware)

## `r2e new <name>` — Project scaffolding

Creates a new R2E project with optional feature selection.

**Flags:**
- `--db <sqlite|postgres|mysql>` — include database support (adds sqlx dep, pool in state, migrations/ dir)
- `--auth` — include JWT/OIDC security (adds `r2e-security`, `JwtClaimsValidator` in state)
- `--openapi` — include OpenAPI documentation (adds `OpenApiPlugin` to builder)
- `--metrics` — reserved for Prometheus metrics (not yet wired)
- `--full` — enable all features (SQLite + auth + openapi + scheduler + events)
- `--no-interactive` — skip interactive prompts, use flags/defaults only

**Interactive mode:** When no flags are provided, uses `dialoguer` to prompt for database and feature selection.

**Generated project uses the `r2e` facade crate** (not `r2e-core` + `r2e-macros` separately). Templates are in `commands/templates/project.rs`.

**Types:**
- `ProjectOptions` — aggregates all feature selections
- `DbKind` — `Sqlite | Postgres | Mysql`
- `CliNewOpts` — raw CLI flag values before resolution

## `r2e generate` — Code generation

Subcommands:

- **`controller <Name>`** — generates `src/controllers/<snake_name>.rs` with a skeleton controller, updates `mod.rs`
- **`service <Name>`** — generates `src/<snake_name>.rs` with a skeleton service struct
- **`crud <Name> --fields "name:Type ..."`** — generates a complete CRUD set:
  - `src/models/<snake>.rs` — entity struct + `Create`/`Update` request types
  - `src/services/<snake>_service.rs` — service with list/get/create/update/delete methods
  - `src/controllers/<snake>_controller.rs` — REST controller with GET/POST/PUT/DELETE endpoints
  - `migrations/<timestamp>_create_<plural>.sql` — SQL migration (if `migrations/` dir exists)
  - `tests/<snake>_test.rs` — integration test skeleton
  - Updates `mod.rs` in each directory
- **`middleware <Name>`** — generates `src/middleware/<snake_name>.rs` with an `Interceptor<R>` impl skeleton, updates `mod.rs`

**Field parsing:** fields are `"name:Type"` pairs (e.g. `"title:String published:bool"`). `Field` struct has `name`, `rust_type`, `is_optional`. SQL type mapping: `String` → `TEXT`, `i64` → `INTEGER`, `f64` → `REAL`, `bool` → `BOOLEAN`.

## `r2e doctor` — Project health diagnostics

Runs 8 checks (Cargo.toml, r2e dep, config file, controllers dir, rustc, dx CLI, migrations, entrypoint). Reports `Ok`/`Warning`/`Error` with colored indicators.

## `r2e routes` — Route listing

Static source parsing of `src/controllers/*.rs` (no compilation). Extracts controller paths, HTTP methods, handler names, roles. Colored table output.

## `r2e dev` — Development server with hot-reload

Uses Dioxus Subsecond for instant hot-patching (no full recompile). Requires `dx` CLI (`cargo install dioxus-cli`). Generates a `Dioxus.toml` config if missing, then runs `dx serve --hot-patch` with the `dev-reload` feature enabled.

**Flags:**
- `--port <PORT>` — server port (forwarded as `R2E_PORT` env var)
- `--features <FEAT>...` — extra Cargo features to enable

**Prerequisites:** `dx` CLI installed. If missing, prints instructions.

## `r2e add <extension>` — Extension management

Adds an R2E sub-crate dependency to `Cargo.toml`. Known extensions: `security`, `data`, `openapi`, `events`, `scheduler`, `cache`, `rate-limit`, `utils`, `prometheus`, `static`, `test`.

## Template system (`commands/templates/`)

Helpers in `templates/mod.rs`: `to_snake_case`, `to_pascal_case`, `pluralize`, `render(template, &[("key", "value")])`.
