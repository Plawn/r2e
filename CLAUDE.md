# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Build all crates
cargo build --workspace

# Check all crates (faster, no codegen)
cargo check --workspace

# Run the example application (serves on 0.0.0.0:3000)
cargo run -p example-app

# Run tests (when added)
cargo test --workspace

# Build a specific crate
cargo build -p quarlus-core
cargo build -p quarlus-macros
cargo build -p quarlus-security

# Expand macros for debugging (requires cargo-expand)
cargo expand -p example-app
```

## Architecture

Quarlus is a **Quarkus-like ergonomic layer over Axum** for Rust. It provides declarative controllers with compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

### Workspace Crates

```
quarlus-macros     → Proc-macro crate (no runtime deps). Parses #[controller] blocks and generates Axum handlers.
quarlus-core       → Runtime foundation. AppBuilder, Controller trait, AppError, Tower layers. Depends on quarlus-macros.
quarlus-security   → JWT validation, JWKS cache, AuthenticatedUser extractor. Depends on quarlus-core.
example-app        → Demo binary using all three crates.
```

Dependency flow: `quarlus-macros` ← `quarlus-core` ← `quarlus-security` ← `example-app`

### Core Concepts

**Two injection scopes, both resolved at compile time:**
- `#[inject]` — App-scoped. Field is cloned from the Axum state (services, repos, pools). Type must be `Clone + Send + Sync`.
- `#[identity]` — Request-scoped. Field is extracted via Axum's `FromRequestParts` (e.g., `AuthenticatedUser` from JWT).

**Controller macro (`#[controller]`)** takes an `impl ControllerName for StateType` block and generates:
1. A struct with `#[inject]` and `#[identity]` fields
2. An impl block with the original methods
3. Free-standing Axum handler functions (named `__quarlus_ControllerName_method_name`)
4. A `Controller<T>` trait impl that wires routes into an `axum::Router<T>`

### Macro Crate Internals (quarlus-macros)

The proc-macro pipeline is: `lib.rs` (entry) → `parsing.rs` (AST → `ControllerDef`) → `codegen.rs` (generate struct, impl, handlers, routes) with `route.rs` defining route/method types.

Key struct: `ControllerDef` in `parsing.rs` holds all parsed fields, identity fields, route methods, and helper methods.

Handler generation pattern: each `#[get("/path")]` method becomes a standalone async function that extracts `State(state)`, any identity extractors, and method parameters, constructs the controller struct, then calls the method.

### Security (quarlus-security)

- `AuthenticatedUser` implements `FromRequestParts` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `#[roles("admin")]` attribute generates a guard that checks `AuthenticatedUser::has_role()` and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default checks top-level `roles` and Keycloak's `realm_access.roles`.

### AppBuilder (quarlus-core)

Fluent API: `AppBuilder::new().with_state(s).with_cors().with_tracing().with_health().register_controller::<C>().build()` returns an `axum::Router`. `.serve(addr)` starts the Tokio server.

### Feature Flags

- `quarlus-core` has an optional `sqlx` feature that enables `sqlx::Error` → `AppError` conversion.
- `#[transactional]` attribute (in macros) wraps a method body in `self.pool.begin()`/`commit()` — requires the controller to have an injected `pool` field.

## Language & Documentation

The project's plan (`plan.md`) and step-by-step docs (`docs/steps/`) are written in French. Code, comments, and API surfaces are in English.
