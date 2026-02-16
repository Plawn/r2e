# Contributing to R2E

Thank you for your interest in contributing to R2E! This guide will help you get set up and familiar with the project.

## Dev setup

1. **Clone the repository**

   ```bash
   git clone https://github.com/plawn/r2e.git
   cd r2e
   ```

2. **Build the workspace**

   ```bash
   cargo build --workspace
   ```

3. **Run tests**

   ```bash
   cargo test --workspace
   ```

**Requirements:** Rust edition 2021 (stable toolchain). No nightly features required.

## Workspace structure

R2E is organized as a Cargo workspace. The dependency flow is bottom-up:

```
r2e-macros (proc-macro, no runtime deps)
    ↑
r2e-core (runtime foundation)
    ↑
r2e-security / r2e-events / r2e-scheduler / r2e-data
    ↑
r2e-data-sqlx / r2e-cache / r2e-rate-limit / r2e-openapi / r2e-utils / r2e-test
    ↑
r2e (facade — re-exports everything, feature-gated)
    ↑
example-app / example-postgres / example-multi-tenant / ...
```

| Crate | Purpose |
|-------|---------|
| `r2e-macros` | Proc macros: `#[derive(Controller)]`, `#[routes]`, `#[bean]`, `#[producer]` |
| `r2e-core` | Runtime: AppBuilder, Controller trait, guards, interceptors, config, plugins |
| `r2e-security` | JWT/OIDC: AuthenticatedUser, JwtValidator, JWKS cache |
| `r2e-events` | In-process typed EventBus with pub/sub |
| `r2e-scheduler` | Background task scheduling (interval, cron) |
| `r2e-data` | Database abstractions: Entity, Repository, QueryBuilder, Pageable/Page |
| `r2e-data-sqlx` | SQLx backend: SqlxRepository, Tx, HasPool, migrations |
| `r2e-cache` | TTL cache with pluggable backends |
| `r2e-rate-limit` | Token-bucket rate limiting |
| `r2e-openapi` | OpenAPI 3.0.3 spec generation + docs UI |
| `r2e-utils` | Built-in interceptors: Logged, Timed, Cache, CacheInvalidate |
| `r2e-test` | TestApp, TestJwt for integration testing |
| `r2e-cli` | CLI scaffolding tool |

For more detail see [`docs/book/src/reference/crate-map.md`](docs/book/src/reference/crate-map.md).

## Proc macro development

The macro crate (`r2e-macros`) has two main code paths:

- **Derive path:** `derive_controller.rs` → `derive_parsing.rs` → `derive_codegen.rs`
- **Routes path:** `routes_attr.rs` → `routes_parsing.rs` → `routes_codegen.rs`

Use `cargo expand` to inspect generated code:

```bash
cargo expand -p example-app controllers::user_controller
```

See the [Macro Debugging guide](docs/book/src/advanced/macro-debugging.md) for details.

## Running tests

```bash
# All tests
cargo test --workspace

# A specific crate
cargo test -p r2e-core

# Compile-fail tests (proc macro error messages)
cargo test -p r2e-compile-tests

# Regenerate compile-fail snapshots after changing error messages
TRYBUILD=overwrite cargo test -p r2e-compile-tests
```

## PR conventions

### Branch naming

- `feat/<description>` — new feature
- `fix/<description>` — bug fix
- `docs/<description>` — documentation only
- `refactor/<description>` — code restructuring

### Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/) style:

```
feat(core): add support for lifecycle hooks
fix(macros): handle unit structs in #[derive(Controller)]
docs: add macro debugging guide
```

### Before submitting

All of the following must pass:

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Good first issues

Look for these labels on the issue tracker:

- **`good-first-issue`** — well-scoped, beginner-friendly tasks
- **`help-wanted`** — contributions welcome, may require more context
- **`docs`** — documentation improvements
- **`proc-macro`** — work in `r2e-macros` (parsing, codegen, error messages)

## Questions?

Open an issue or start a discussion on the repository. We're happy to help!
