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
/// `R2E_PROFILE=dev`, route listing before start).
pub mod dev;

/// Project diagnostics — `r2e doctor`.
///
/// Runs 8 health checks: Cargo.toml, R2E dependency, config file,
/// controllers directory, Rust toolchain, cargo-watch, migrations,
/// and application entrypoint.
pub mod doctor;

/// Code generation — `r2e generate`.
///
/// Subcommands: `controller`, `service`, `crud`, `middleware`, `grpc-service`.
/// Generates skeleton source files and updates `mod.rs` declarations.
pub mod generate;

/// Project scaffolding — `r2e new <name>`.
///
/// Creates a new R2E project directory with Cargo.toml, main.rs, state.rs,
/// application.yaml, and optional database/auth/openapi/gRPC scaffolding.
pub mod new_project;

/// Route listing — `r2e routes`.
///
/// Static source parsing of `src/controllers/*.rs` to extract declared
/// routes, HTTP methods, handler names, and role annotations.
pub mod routes;

/// Shared template helpers and code templates.
///
/// Provides string utilities (`to_snake_case`, `to_pascal_case`, `pluralize`,
/// `render`) and code generation templates for projects and middleware.
pub mod templates;
