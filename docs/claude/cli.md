# CLI (r2e-cli)

The `r2e` binary provides project scaffolding, code generation, diagnostics, and development tooling.

## `r2e new <name>` — Project scaffolding

Creates a new R2E project with optional feature selection.

**Flags:**
- `--db <sqlite|postgres|mysql>` — include database support (adds sqlx dep, pool in state, migrations/ dir)
- `--auth` — include JWT/OIDC security (adds `r2e-security`, `JwtClaimsValidator` in state)
- `--openapi` — include OpenAPI documentation (adds `OpenApiPlugin` to builder)
- `--metrics` — reserved for Prometheus metrics (not yet wired)
- `--grpc` — include gRPC server support (proto automagic: one-line `build.rs` via `r2e-grpc-build`, `proto/greeter.proto`, `src/grpc/` skeleton with `include_protos!()`)
- `--full` — enable all features (SQLite + auth + openapi + scheduler + events + gRPC)
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
- **`grpc-service <Name> [--package <pkg>]`** — generates `proto/<snake>.proto` (with `Get<Name>`/`List<Name>` RPCs) + `src/grpc/<snake>.rs` (a `#[grpc_routes]` controller wired to `super::proto`), creating the shared `src/grpc/mod.rs` (`include_protos!()`) if missing and updating it. `--package` sets the protobuf package (default `myapp`).

**Field parsing:** fields are `"name:Type"` pairs (e.g. `"title:String published:bool"`). `Field` struct has `name`, `rust_type`, `is_optional`. SQL type mapping: `String` → `TEXT`, `i64` → `INTEGER`, `f64` → `REAL`, `bool` → `BOOLEAN`.

## `r2e doctor` — Project health diagnostics

Runs 10 checks (Cargo.toml, r2e dep, config file, controllers dir, rustc, dx CLI, migrations, R2E entrypoint, DI recursion limit, exported agent docs `docs/r2e/llm.txt` present and stamped with the CLI's version). The entrypoint check recognizes `app_main!`, `launch!`, `serve()`, and `serve_auto()`. Reports `Ok`/`Warning`/`Error` with colored indicators.

## `r2e routes` — Route listing

Static source parsing of `src/controllers/*.rs` (no compilation). Extracts controller paths, HTTP methods, handler names, roles. Colored table output.

It also scans all of `src/` for `#[module(prefix = "…", controllers(...))]` and prefixes the rows of every controller a module mounts, so the printed path is the served path (a `#[fallback]` in a prefixed module prints as `/prefix/*`). Still purely textual — the CLI never builds the app — so a controller listed in a module declared outside `src/` keeps its unprefixed path.

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

## `r2e docs --llm [<topic>]` — AI/agent-facing reference

The same delivery for the hub + spokes reference (`llm.txt` + `llm/<topic>.md` at the repo root, see `plans/llm-docs-split.md`). Embedded with `include_str!` like the module docs, so the printed/exported text is the one for the installed `r2e` version.

- **`r2e docs --llm`** — the hub: golden rules + routing table "task → topic".
- **`r2e docs --llm <topic>`** — one topic file, front matter included (`--pretty` strips it and renders).
- **`--full`** — hub + every topic in routing order, one document (what `llm-full.txt` is in the repo), stamped with the version.
- **`--export [DIR]`** — writes `DIR/llm.txt` (stamped `<!-- R2E vX.Y.Z — exported by … -->`) and `DIR/llm/<topic>.md`; default `DIR` is `docs/r2e`. `r2e new` runs this export for every scaffolded project and writes `.claude/skills/r2e/SKILL.md` pointing at it; the generated `AGENTS.md` routes agents to `docs/r2e/llm.txt`. `r2e doctor` (check 10) warns when `docs/r2e/llm.txt` is missing or its stamp is not the CLI's version.

Implementation: `commands/llm_docs.rs` (`HUB`, `TOPICS` — one entry per `llm/*.md`, kept in sync by `tests/llm_docs.rs`, `full()`, `export()`, `stamped_version()`).

## `r2e dev` — Development server with hot-reload

Uses Dioxus Subsecond for instant hot-patching — recompiles only changed code as a dynamic library and patches it into the running process (~200-500ms). Requires `dx` CLI (`cargo install dioxus-cli`). Generates a `Dioxus.toml` config if missing, then runs `dx serve --hot-patch` with the `dev-reload` feature enabled.

**Flags:**
- `--port <PORT>` — server port (forwarded as `R2E_PORT` env var)
- `--features <FEAT>...` — extra Cargo features to enable

**Prerequisites:** `dx` CLI installed. If missing, prints instructions.

## `r2e add <extension>` — Extension management

Adds an R2E extension to `Cargo.toml`. Known extensions: `security`, `data-sqlx`, `data-diesel`, `openapi`, `events`, `scheduler`, `cache`, `rate-limit`, `utils`, `prometheus`, `grpc`, `mcp`, `test`. (`r2e add openapi` also pulls in `schemars`.)

**`r2e add mcp` is a full, idempotent scaffold**: when an `r2e` dependency exists, it enables its `mcp` feature; without the facade it falls back to direct `r2e-mcp`. It ensures `schemars` plus `serde/derive`, creates `src/mcp.rs` with a dedicated `#[controller]` + `#[mcp_routes]` adapter when no MCP module exists, and adds a minimal `mcp:` block to `application.yaml` when absent. Existing source/config are never overwritten; the command prints the `McpServer` + `.register_mcp_service::<McpTools>()` App wiring.

**`r2e add grpc` is a full scaffold**, not just a dependency insert (`scaffold_grpc` in `commands/add.rs`): enables the `grpc`/`grpc-reflection` features on the `r2e` facade dep (converting a bare version string to an inline table if needed; falls back to a direct `r2e-grpc` dep when there is no `r2e` dep), adds `tonic`/`tonic-prost`/`prost` (~0.14) and the `r2e-grpc-build` build-dependency (mirroring the `r2e` dep's git source incl. branch/rev/tag), writes the one-line `build.rs` (never overwrites an existing one), and — only when the project has no `.proto` yet — drops `proto/greeter.proto` plus a `src/grpc/` module skeleton, then prints the `App::build` wiring snippet. All three scaffolding paths (`r2e new --grpc`, `r2e add grpc`, `r2e generate grpc-service`) share one layout: `src/grpc/mod.rs` owns the single `pub mod proto { include_protos!() }`, service files live next to it and use `super::proto`.

## Template system (`commands/templates/`)

Helpers in `templates/mod.rs`: `to_snake_case`, `to_pascal_case`, `pluralize`, `render(template, &[("key", "value")])`.

## Key files

- `r2e-cli/src/main.rs` — CLI entry point (clap `Commands` + `GenerateKind` enums)
- `r2e-cli/src/commands/` — one module per command
- `r2e-cli/src/commands/templates/` — code generation templates (project, middleware)
