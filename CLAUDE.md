# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build --workspace            # Build all crates
cargo check --workspace            # Check all crates (faster, no codegen)
cargo run -p example-app           # Run the example app (serves on 0.0.0.0:3000)
cargo test --workspace             # Run tests
cargo build -p <crate-name>        # Build a specific crate
cargo expand -p example-app        # Expand macros (requires cargo-expand)
```

## Testing Conventions

**Tests live in `<crate>/tests/` directories, not inline.** Do NOT use `#[cfg(test)] mod tests { ... }` blocks inside source files.

- One test file per source module: `src/foo.rs` → `tests/foo.rs`
- Use external imports (`use <crate_name>::...`) instead of `use super::*`
- Keep test-only helpers in the test file, not in the source
- If a test needs access to an internal item, add a `pub` accessor or `pub` + `#[doc(hidden)]` — do NOT use `#[cfg(test)] pub(crate)` visibility hacks
- Feature-gated modules need `#![cfg(feature = "...")]` at the top of the test file

```bash
cargo test --workspace                    # all tests
cargo test -p r2e-core                    # single crate
cargo test -p r2e-core --test config      # single test file
```

## Architecture

R2E is a **Quarkus-like ergonomic layer over Axum** for Rust. It provides declarative controllers with compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

### Workspace Crates

```
r2e-macros      → Proc-macro crate. #[derive(Controller)] + #[routes] generate Axum handlers.
r2e-core        → Runtime foundation. AppBuilder, Controller trait, StatefulConstruct, HttpError, Guard, Interceptor, R2eConfig, lifecycle hooks.
r2e-security    → JWT validation, JWKS cache, AuthenticatedUser extractor, RoleExtractor trait.
r2e-events      → In-process EventBus with typed pub/sub (emit, emit_and_wait, subscribe).
r2e-scheduler   → Background task scheduling (interval, cron, initial delay). CancellationToken-based shutdown.
r2e-data        → Data access abstractions: Entity, Repository, Page, Pageable, DataError.
r2e-data-sqlx   → SQLx backend: SqlxRepository, Tx, HasPool, ManagedResource impl, migrations.
r2e-data-diesel → Diesel backend (skeleton): DieselRepository, error bridge.
r2e-cache       → TtlCache, pluggable CacheStore trait (default InMemoryStore).
r2e-rate-limit  → Token-bucket RateLimiter, pluggable RateLimitBackend, RateLimitRegistry.
r2e-openapi     → OpenAPI 3.0.3 spec generation, Swagger UI at /docs.
r2e-utils       → Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
r2e-test        → TestApp (HTTP client wrapper), TestJwt (JWT generation for tests).
r2e-devtools    → Subsecond hot-reload support (wraps dioxus-devtools). Feature-gated behind `dev-reload`.
r2e-cli         → CLI: r2e new, r2e add, r2e dev, r2e generate, r2e doctor, r2e routes.
example-app     → Demo binary exercising all features.
```

Dependency flow: `r2e-macros` ← `r2e-core` ← `r2e-security` / `r2e-events` / `r2e-scheduler` / `r2e-data` / `r2e-devtools` ← `r2e-data-sqlx` / `r2e-data-diesel` / `r2e-cache` / `r2e-rate-limit` / `r2e-openapi` / `r2e-utils` / `r2e-test` ← `example-app`

### Vendored Dependencies

`vendor/openfga-rs/` — patched copy using tonic ~0.12 with `features = ["tls", "channel", "codegen", "prost"]` to avoid dual axum-core conflict. See `vendor/README.md`.

### Core Concepts

**Three injection scopes, all resolved at compile time:**
- `#[inject]` — App-scoped. Field cloned from Axum state. Type must be `Clone + Send + Sync`.
- `#[inject(identity)]` — Request-scoped. Extracted via `FromRequestParts` (e.g., `AuthenticatedUser`). Type must implement `Identity`.
- `#[config("key")]` — App-scoped. Resolved from `R2eConfig`. Type must implement `FromConfigValue`.

**Handler parameter-level identity injection:**
- `#[inject(identity)]` on handler parameters enables mixed controllers (public + protected endpoints) while keeping `StatefulConstruct` for consumers/scheduled tasks.
- **Optional identity:** `#[inject(identity)] user: Option<AuthenticatedUser>` for endpoints working with or without auth.

**Controller declaration uses two macros:**
1. `#[derive(Controller)]` on the struct — generates metadata module, Axum extractor, `StatefulConstruct` impl (when no identity fields).
2. `#[routes]` on the impl block — generates Axum handler functions and `Controller<T>` trait impl.

```rust
#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject]  user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }
}
```

**Generated items (hidden):**
- `mod __r2e_meta_<Name>` — `type State`, `type IdentityType`, `const PATH_PREFIX`, `fn guard_identity()`
- `struct __R2eExtract_<Name>` — `FromRequestParts` extractor
- `impl StatefulConstruct<State> for Name` — only when no `#[inject(identity)]` struct fields
- Free-standing Axum handler functions (`__r2e_<Name>_<method>`)
- `impl Controller<State> for Name` — wires routes into `axum::Router<State>`

### Macro Crate Internals (r2e-macros)

**Derive path:** `lib.rs` → `derive_controller.rs` → `derive_parsing.rs` (`ControllerStructDef`) → `derive_codegen.rs`

**Routes path:** `lib.rs` → `routes_attr.rs` → `routes_parsing.rs` (`RoutesImplDef`) → `routes_codegen.rs`

**Shared modules:**
- `types.rs` — `InjectedField`, `IdentityField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.
- `attr_extract.rs` — `extract_route_attr`, `extract_roles`, `extract_transactional`, `extract_intercept_fns`, etc.
- `route.rs` — `HttpMethod` enum and `RoutePath` parser

**Inter-macro liaison:** Derive generates `__r2e_meta_<Name>` + `__R2eExtract_<Name>`. `#[routes]` references these by naming convention.

**No-op attribute macros:** `#[get]`, `#[roles]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, etc. are no-op `#[proc_macro_attribute]` parsed by `#[routes]`. `#[inject]`, `#[identity]`, `#[config]` are derive helper attributes.

## Detailed Reference (see linked files)

- **[Configuration](docs/claude/configuration.md)** — R2eConfig, ConfigProperties, secrets, profiles, validation, FromConfigValue, typed sections, registry
- **[Guards & Interceptors](docs/claude/guards-interceptors.md)** — Guard/PreAuthGuard traits, GuardContext, Identity, RolesGuard, RateLimitGuard, interceptor wrapping order, configurable syntax
- **[Error Handling & Managed Resources](docs/claude/error-handling.md)** — HttpError variants, `#[derive(ApiError)]`, `map_error!`, validation, ManagedResource trait, `#[managed]`
- **[Beans & Dependency Injection](docs/claude/beans-di.md)** — Bean/AsyncBean/Producer traits, `#[bean]`, `#[producer]`, `#[config]` in beans, `build_state()`
- **[Subsystems](docs/claude/subsystems.md)** — Cache, Rate Limiting, Security, Events, Scheduling, Data, OpenAPI, StatefulConstruct, AppBuilder, Testing, Configuration
- **[Prelude & Feature Flags](docs/claude/prelude-features.md)** — Full prelude listing, `use r2e::prelude::*`, validation/garde, Params, transactional
- **[CLI](docs/claude/cli.md)** — `r2e new`, `r2e generate`, `r2e doctor`, `r2e routes`, `r2e dev`, `r2e add`, templates

## Language & Documentation

The project's plan (`plan.md`) and step-by-step docs (`docs/steps/`) are written in French. Code, comments, and API surfaces are in English.

