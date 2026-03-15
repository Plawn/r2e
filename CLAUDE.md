# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Status

R2E is **not in production yet**. Breaking changes are always allowed — no need to gate them behind feature flags or maintain backward compatibility. Just mention breaking changes explicitly in plans so they are acknowledged.

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
r2e-core        → Runtime foundation. AppBuilder (load_config, with_config, serve_auto), Controller trait, StatefulConstruct, PostConstruct, HttpError, Guard, Interceptor, R2eConfig, lifecycle hooks.
r2e-security    → JWT validation, JWKS cache, AuthenticatedUser extractor, RoleExtractor trait.
r2e-events      → In-process EventBus with typed pub/sub (emit, emit_and_wait, subscribe).
r2e-events-iggy → Apache Iggy EventBus backend: persistent distributed event streaming.
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
r2e-static      → Embedded static file serving with SPA support. Plugin-based, wraps rust_embed.
r2e-cli         → CLI: r2e new, r2e add, r2e dev, r2e generate, r2e doctor, r2e routes.
example-app     → Demo binary exercising all features.
```

Dependency flow: `r2e-macros` ← `r2e-core` ← `r2e-security` / `r2e-events` / `r2e-scheduler` / `r2e-data` / `r2e-devtools` / `r2e-static` ← `r2e-events-iggy` / `r2e-data-sqlx` / `r2e-data-diesel` / `r2e-cache` / `r2e-rate-limit` / `r2e-openapi` / `r2e-utils` / `r2e-test` ← `example-app`

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

**No-op attribute macros:** `#[get]`, `#[roles]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, `#[post_construct]`, etc. are no-op `#[proc_macro_attribute]` parsed by `#[routes]` or `#[bean]`. `#[inject]`, `#[identity]`, `#[config]` are derive helper attributes.

**`#[post_construct]`** — lifecycle hook on `#[bean]` methods. Called after the entire bean graph is resolved. `&self` only, may be async, returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`. Generates `PostConstruct` trait impl.

## Detailed Reference — Read Before You Code

**DO NOT guess APIs or patterns. Match your task to the keyword table below and READ only the matching file(s).** Each file is the authoritative source for its subsystem. Reading all files wastes context — be selective.

### Keyword → Doc routing table

| If your task involves… | Read this file |
|---|---|
| `R2eConfig`, `ConfigProperties`, `ConfigValue`, `FromConfigValue`, `#[config(...)]`, `load_config`, `with_config`, secrets (`${...}`), YAML config, typed sections, `#[config(section)]`, env overlay, `serve_auto` | `docs/claude/configuration.md` |
| `Guard`, `PreAuthGuard`, `GuardContext`, `#[guard]`, `#[roles]`, `Identity`, `RolesGuard`, `RateLimitGuard`, `Interceptor`, `#[intercept]`, `Logged`, `Timed`, middleware ordering | `docs/claude/guards-interceptors.md` |
| `HttpError`, `ApiError`, `#[derive(ApiError)]`, `map_error!`, validation, `garde`, `ManagedResource`, `#[managed]`, error responses | `docs/claude/error-handling.md` |
| `Bean`, `AsyncBean`, `Producer`, `#[bean]`, `#[producer]`, `#[inject]`, `#[post_construct]`, `BeanRegistry`, `BeanContext`, `build_state`, dependency injection, bean graph | `docs/claude/beans-di.md` |
| `Cache`, `TtlCache`, `RateLimiter`, `RateLimitRegistry`, `AuthenticatedUser`, `JwksValidator`, `EventBus`, `#[consumer]`, `#[scheduled]`, `Scheduler`, `Repository`, `Entity`, `OpenAPI`, `StatefulConstruct`, `AppBuilder`, `TestApp`, `TestJwt`, `TracingConfig`, `LogFormat`, `SpanEvents`, `ConfiguredTracing`, `init_tracing_with_config`, tracing subscriber formatting | `docs/claude/subsystems.md` |
| `prelude`, `use r2e::prelude::*`, feature flags, `Params`, `#[transactional]`, re-exports, what's available by default | `docs/claude/prelude-features.md` |
| `r2e new`, `r2e dev`, `r2e generate`, `r2e add`, `r2e doctor`, `r2e routes`, CLI templates, scaffolding | `docs/claude/cli.md` |

**Rules:**
1. Match keywords from your task to the left column. Read **only** the matched file(s).
2. If your task spans two subsystems (e.g., config + beans), read both — but no more.
3. If nothing matches, you probably don't need a reference doc. Proceed with the code.

## Language & Documentation

All documentation, code, comments, and API surfaces are in English.

