//! Command implementations for the `r2e` CLI.
//!
//! Each submodule corresponds to a top-level CLI command.

/// Extension management — `r2e add <extension>`.
///
/// Adds an R2E sub-crate dependency to the project's `Cargo.toml`.
/// Known extensions: security, data, data-sqlx, data-diesel, openapi,
/// events, scheduler, cache, rate-limit, utils, prometheus, grpc, test.
pub mod add;

/// Development server — `r2e dev`.
///
/// Wraps `cargo watch` with R2E-specific defaults (watched paths,
/// route listing before start).
pub mod dev;

/// Module documentation — `r2e docs [<module>]`.
///
/// Prints bundled, version-matched per-module docs (the `docs/features/*.md`
/// set, embedded at compile time). Defaults to the curated `## TL;DR` section;
/// `--full` prints the whole document, `--pretty` renders markdown for a terminal.
pub mod docs;

/// Project diagnostics — `r2e doctor`.
///
/// Runs 10 health checks: Cargo.toml, R2E dependency, config file,
/// controllers directory, Rust toolchain, Dioxus CLI, migrations,
/// application entrypoint, bean-count recursion limits, and the exported
/// agent docs (`docs/r2e/`) being present and version-matched.
pub mod doctor;

/// AI/agent-facing reference — `r2e docs --llm [<topic>] [--full] [--export [DIR]]`.
///
/// The hub `llm.txt` (golden rules + routing table) and the per-topic files
/// under `llm/` are embedded at compile time, so the printed or exported
/// reference always matches the installed R2E version. `r2e new` exports the
/// set into `docs/r2e/`; `r2e doctor` warns when that copy is stale.
pub mod llm_docs;

/// Code generation — `r2e generate`.
///
/// Subcommands: `controller`, `service`, `crud`, `middleware`, `grpc-service`.
/// Generates skeleton source files and updates `mod.rs` declarations.
pub mod generate;

/// Project scaffolding — `r2e new <name>`.
///
/// Creates a new R2E project directory with Cargo.toml, app.rs, env.rs, lib.rs,
/// main.rs, application.yaml, and optional database/auth/openapi/gRPC scaffolding.
pub mod new_project;

/// Route listing — `r2e routes`.
///
/// Static source parsing of `src/controllers/*.rs` to extract declared
/// routes, HTTP methods, handler names, and role annotations.
pub mod routes;

/// Test runner — `r2e test`.
///
/// Wraps `cargo test` and can generate `cargo llvm-cov` coverage reports,
/// including LCOV output consumable by SonarQube.
pub mod test;

/// Shared template helpers and code templates.
///
/// Provides string utilities (`to_snake_case`, `to_pascal_case`, `pluralize`,
/// `render`) and code generation templates for projects and middleware.
pub mod templates;
