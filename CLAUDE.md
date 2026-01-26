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

**No-op attribute macros:** `lib.rs` declares attributes like `#[get]`, `#[logged]`, `#[cached]`, etc. as no-op `#[proc_macro_attribute]` that return their input unchanged. These are **not** consumed at the attribute-macro level — they are parsed from the raw token stream inside the `controller!` function-like macro by `parsing.rs`. The no-op declarations exist for three reasons: (1) they prevent "cannot find attribute" compiler errors if someone uses them outside `controller!`, (2) they appear in `cargo doc` with their documentation, making the API discoverable, and (3) they give IDEs like rust-analyzer autocomplete and hover support. The `controller!` macro would compile fine without them.

### Interceptors

Cross-cutting concerns (logging, timing, caching) are implemented via a generic `Interceptor<R>` trait with an `around` pattern (`quarlus-core/src/interceptors.rs`). All calls are monomorphized (no `dyn`) for zero overhead.

**Built-in interceptors:**
- `Logged` — logs entry/exit at a configurable `LogLevel`.
- `Timed` — measures execution time, with an optional threshold (only logs if exceeded).
- `Cached` — caches `Json<T>` responses in a `TtlCache<String, String>`. Requires `T: Serialize + DeserializeOwned`. Only works with `Json<T>` return types (not `Result<Json<T>, AppError>`).

**Interceptor wrapping order** (outermost → innermost):

Handler level (before the controller, in `generate_single_handler`):
1. `rate_limited` — short-circuits with 429
2. `roles` — short-circuits with 403

Method body level (trait-based, via `Interceptor::around`, in `generate_wrapped_method`):
3. `logged`
4. `timed`
5. User-defined interceptors (`#[intercept(...)]`)
6. `cached`

Inline codegen (no trait):
7. `cache_invalidate` (after body)
8. `transactional` (wraps body in tx begin/commit)
9. Original method body

**Configurable syntax:**
```rust
#[logged]                                    // default: Info
#[logged(level = "debug")]                   // custom level
#[timed]                                     // default: Info, no threshold
#[timed(level = "warn", threshold = 100)]    // only log if > 100ms
#[cached(ttl = 30)]                          // anonymous static cache
#[cached(ttl = 30, group = "users")]         // named cache group (for invalidation)
#[cached(ttl = 30, key = "params")]          // key by method params (requires Debug)
#[cached(ttl = 30, key = "user")]            // key by identity.sub (requires #[identity])
#[cache_invalidate("users")]                 // clears a named cache group after method runs
#[transactional]                             // uses self.pool
#[transactional(pool = "read_db")]           // custom pool field
#[rate_limited(max = 5, window = 60)]                  // global key
#[rate_limited(max = 5, window = 60, key = "user")]    // per-user (requires #[identity])
#[rate_limited(max = 5, window = 60, key = "ip")]      // per-IP (X-Forwarded-For)
#[intercept(MyInterceptor)]                  // user-defined (must be a unit struct/constant)
```

**User-defined interceptors** implement `Interceptor<R>` and are applied via `#[intercept(TypeName)]`. The type must be constructable as a bare path expression (unit struct or constant).

**Cache infrastructure:** `CacheRegistry` (`quarlus-core/src/cache.rs`) is a global static registry of named `TtlCache` instances. `#[cached(group = "x")]` stores in the registry; `#[cache_invalidate("x")]` clears it. TTL is set by whichever method first creates the group.

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
