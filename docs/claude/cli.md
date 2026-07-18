# CLI (r2e-cli)

The `r2e` binary provides project scaffolding, code generation, diagnostics, and development tooling.

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

Runs 9 checks (Cargo.toml, r2e dep, config file, controllers dir, rustc, dx CLI, migrations, R2E entrypoint, DI recursion limit). The entrypoint check recognizes `app_main!`, `launch!`, `serve()`, and `serve_auto()`. Reports `Ok`/`Warning`/`Error` with colored indicators.

## `r2e routes` — Route listing

Static source parsing of `src/controllers/*.rs` (no compilation). Extracts controller paths, HTTP methods, handler names, roles. Colored table output.

## `r2e docs [<module>]` — Bundled module documentation

Prints per-module documentation embedded in the binary at compile time (the `docs/features/*.md` set, via `include_str!`), so it is always version-matched to the installed `r2e`. Aimed at both agents (raw markdown on stdout, injectable into context) and humans (`--pretty`).

- **No argument** — lists every module: `slug — Title (crate[, crate])`.
- **`r2e docs <slug>`** — prints the curated `## TL;DR` section of that module (e.g. `events`, `security`, `configuration`).
- **Crate-name alias** — `r2e docs r2e-events` resolves to the module owned by that crate. A crate owning several modules (e.g. `r2e-core`) **lists** them instead of printing one.
- **Unknown name** — errors with the list of available slugs (exit 1).

**Flags:**
- `--full` — print the whole document instead of just the TL;DR.
- `--pretty` / `-p` — render markdown for the terminal (via `termimad`) instead of raw output.

**Source of truth:** the `## TL;DR` block lives once in each `docs/features/NN-*.md` file — it renders in the docs/mdBook *and* is extracted by this command (slice from `## TL;DR` to the next `## ` heading). Slugs are clean English, decoupled from the (sometimes French) file names. Implementation: `commands/docs.rs` (`DOCS` manifest + `tldr()` extractor).

> **Packaging note:** `include_str!` reads `../../../docs/features/*.md`, outside the `r2e-cli` crate dir. This works for in-workspace builds; publishing `r2e-cli` to crates.io will need the docs mirrored under the crate (or a `build.rs`) first.

## `r2e dev` — Development server with hot-reload

Uses Dioxus Subsecond for instant hot-patching — recompiles only changed code as a dynamic library and patches it into the running process (~200-500ms). Requires `dx` CLI (`cargo install dioxus-cli`). Generates a `Dioxus.toml` config if missing, then runs `dx serve --hot-patch` with the `dev-reload` feature enabled.

**Flags:**
- `--port <PORT>` — server port (forwarded as `R2E_PORT` env var)
- `--features <FEAT>...` — extra Cargo features to enable

**Prerequisites:** `dx` CLI installed. If missing, prints instructions.

## `r2e add <extension>` — Extension management

Adds an R2E sub-crate dependency to `Cargo.toml`. Known extensions: `security`, `data`, `openapi`, `events`, `scheduler`, `cache`, `rate-limit`, `utils`, `prometheus`, `static`, `test`.

## Template system (`commands/templates/`)

Helpers in `templates/mod.rs`: `to_snake_case`, `to_pascal_case`, `pluralize`, `render(template, &[("key", "value")])`.

## Key files

- `r2e-cli/src/main.rs` — CLI entry point (clap `Commands` + `GenerateKind` enums)
- `r2e-cli/src/commands/` — one module per command
- `r2e-cli/src/commands/templates/` — code generation templates (project, middleware)
