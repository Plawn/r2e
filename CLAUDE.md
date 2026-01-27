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
quarlus-macros     → Proc-macro crate (no runtime deps). #[derive(Controller)] + #[routes] generate Axum handlers.
quarlus-core       → Runtime foundation. AppBuilder, Controller trait, StatefulConstruct trait, AppError, Tower layers. Depends on quarlus-macros.
quarlus-security   → JWT validation, JWKS cache, AuthenticatedUser extractor. Depends on quarlus-core.
example-app        → Demo binary using all three crates.
```

Dependency flow: `quarlus-macros` ← `quarlus-core` ← `quarlus-security` ← `example-app`

### Core Concepts

**Two injection scopes, both resolved at compile time:**
- `#[inject]` — App-scoped. Field is cloned from the Axum state (services, repos, pools). Type must be `Clone + Send + Sync`.
- `#[identity]` — Request-scoped. Field is extracted via Axum's `FromRequestParts` (e.g., `AuthenticatedUser` from JWT).

**Controller declaration uses two macros working together:**

1. `#[derive(Controller)]` on the struct — generates metadata module, Axum extractor, and `StatefulConstruct` impl (when no identity fields).
2. `#[routes]` on the impl block — generates Axum handler functions, `Controller<T>` trait impl, and `ScheduledController<T>` impl.

```rust
#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject]  user_service: UserService,
    #[identity] user: AuthenticatedUser,
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
- `mod __quarlus_meta_<Name>` — contains `type State`, `const PATH_PREFIX`, `fn guard_identity()`
- `struct __QuarlusExtract_<Name>` — `FromRequestParts` extractor that constructs the controller from state + request parts
- `impl StatefulConstruct<State> for Name` — only when no `#[identity]` fields; used by consumers and scheduled tasks
- Free-standing Axum handler functions (named `__quarlus_<Name>_<method>`)
- `impl Controller<State> for Name` — wires routes into `axum::Router<State>`

### Macro Crate Internals (quarlus-macros)

The proc-macro pipeline has two entry points:

**Derive path:** `lib.rs` → `derive_controller.rs` → `derive_parsing.rs` (DeriveInput → `ControllerStructDef`) → `derive_codegen.rs` (generate meta module, extractor, StatefulConstruct)

**Routes path:** `lib.rs` → `routes_attr.rs` → `routes_parsing.rs` (ItemImpl → `RoutesImplDef`) → `routes_codegen.rs` (generate impl block, handlers, Controller trait impl, ScheduledController impl)

**Shared modules:**
- `types.rs` — shared types (`InjectedField`, `IdentityField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.)
- `attr_extract.rs` — attribute extraction functions (`extract_route_attr`, `extract_roles`, `extract_transactional`, `extract_intercept_fns`, etc.)
- `route.rs` — `HttpMethod` enum and `RoutePath` parser

**Inter-macro liaison:** The derive generates a hidden module `__quarlus_meta_<Name>` and an extractor struct `__QuarlusExtract_<Name>`. The `#[routes]` macro references these by naming convention.

Handler generation pattern: each `#[get("/path")]` method becomes a standalone async function that takes `__QuarlusExtract_<Name>` (which implements `FromRequestParts`) and method parameters. The extractor constructs the controller from state + request parts. For guarded handlers, `State(state)` and `HeaderMap` are also extracted.

**No-op attribute macros:** `lib.rs` declares attributes like `#[get]`, `#[roles]`, `#[intercept]`, etc. as no-op `#[proc_macro_attribute]` that return their input unchanged. These are parsed from the token stream by `#[routes]`. The no-op declarations exist for: (1) preventing "cannot find attribute" errors outside `#[routes]`, (2) `cargo doc` visibility, (3) IDE autocomplete support. The `#[inject]`, `#[identity]`, and `#[config]` attributes are derive helper attributes (consumed by `#[derive(Controller)]`).

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
#[transactional]                             // uses self.pool
#[transactional(pool = "read_db")]           // custom pool field
#[rate_limited(max = 5, window = 60)]                  // global key
#[rate_limited(max = 5, window = 60, key = "user")]    // per-user (requires #[identity])
#[rate_limited(max = 5, window = 60, key = "ip")]      // per-IP (X-Forwarded-For)
#[intercept(MyInterceptor)]                  // user-defined (must be a unit struct/constant)
#[intercept(Logged::info())]                 // built-in interceptor with config
#[intercept(Cache::ttl(30).group("users"))]  // cache with named group
#[intercept(CacheInvalidate::group("users"))] // invalidate cache group
```

**User-defined interceptors** implement `Interceptor<R>` and are applied via `#[intercept(TypeName)]`. The type must be constructable as a bare path expression (unit struct or constant).

**Cache infrastructure:** `CacheRegistry` (`quarlus-core/src/cache.rs`) is a global static registry of named `TtlCache` instances. `#[cached(group = "x")]` stores in the registry; `#[cache_invalidate("x")]` clears it. TTL is set by whichever method first creates the group.

### Security (quarlus-security)

- `AuthenticatedUser` implements `FromRequestParts` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `#[roles("admin")]` attribute generates a guard that checks `AuthenticatedUser::has_role()` and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default checks top-level `roles` and Keycloak's `realm_access.roles`.

### StatefulConstruct (quarlus-core)

`StatefulConstruct<S>` trait allows constructing a controller from state alone (no HTTP context). Auto-generated by `#[derive(Controller)]` when the struct has no `#[identity]` fields. Used by:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Controllers with `#[identity]` fields do NOT get this impl. Attempting to use them in consumer/scheduled context produces a compile error with a diagnostic message via `#[diagnostic::on_unimplemented]`.

### AppBuilder (quarlus-core)

Fluent API: `AppBuilder::new().with_state(s).with_cors().with_tracing().with_health().register_controller::<C>().build()` returns an `axum::Router`. `.serve(addr)` starts the Tokio server.

### Feature Flags

- `quarlus-core` has an optional `sqlx` feature that enables `sqlx::Error` → `AppError` conversion.
- `#[transactional]` attribute (in macros) wraps a method body in `self.pool.begin()`/`commit()` — requires the controller to have an injected `pool` field.

## Language & Documentation

The project's plan (`plan.md`) and step-by-step docs (`docs/steps/`) are written in French. Code, comments, and API surfaces are in English.
