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

**Requirements:** Rust edition 2024, stable toolchain, 1.94 or newer (the MSRV is
declared once in `[workspace.package]` and checked in CI). No nightly features
required — `rust-toolchain.toml` pins a nightly only because the trybuild
`.stderr` expectations in `r2e-compile-tests` are blessed on one.

## Workspace structure

R2E is organized as a Cargo workspace. The dependency flow is bottom-up:

```
r2e-rt (async-runtime facade, the only crate naming tokio)   r2e-tracing (global subscriber)
    ↑                                                              │
r2e-http (HTTP abstraction, the only crate depending on axum)       │
    ↑                                                              │
r2e-macros (proc macros)                                           │
    ↑                                                              │
r2e-core (runtime foundation) ◄────────────────────────────────────┘
    ↑
integrations: r2e-security, r2e-events (+ backends), r2e-scheduler, r2e-executor,
              r2e-data-sqlx, r2e-data-diesel, r2e-cache, r2e-rate-limit, r2e-openapi,
              r2e-prometheus, r2e-observability, r2e-grpc, r2e-mcp, r2e-oidc,
              r2e-openfga, r2e-tenant, r2e-static, r2e-utils, r2e-devtools, r2e-test, …
    ↑
r2e (facade — re-exports everything, feature-gated)
    ↑
applications: examples/*, r2e-cli templates
```

`r2e-http` does not depend on `r2e-tracing`; `r2e-core` is its first consumer.
Some integrations depend on each other (`r2e-scheduler` runs on the
`r2e-executor` pool, the data backends depend on `r2e-tenant` behind their
`tenant` feature, `r2e-mcp` uses `r2e-security`), but none depends on the
facade outside dev-dependencies. The axum,
tokio and JSON-codec containment rules are enforced by
`scripts/check-dep-boundary.sh` and `scripts/check-source-boundary.sh`.

| Crate | Purpose |
|-------|---------|
| `r2e` | Facade: re-exports the subcrates behind feature flags; what applications depend on |
| `r2e-rt` | Async-runtime facade (`r2e_core::rt`); the only crate naming tokio |
| `r2e-tracing` | Process-global tracing subscriber: `TracingOptions`, one-shot install, boot-log buffer |
| `r2e-http` | HTTP abstraction; sole owner of axum; `IntoHttpResponse`, JSON codec façade, QUIC/HTTP3 |
| `r2e-macros` | Proc macros: `#[controller]`, `#[routes]`, `#[bean]`, `#[producer]`, `#[module]`, derives |
| `r2e-core` | Runtime: AppBuilder, Controller trait, guards, interceptors, config, plugins, lifecycle |
| `r2e-security` | JWT/OIDC: AuthenticatedUser, JwtValidator, JWKS cache |
| `r2e-events` | In-process typed EventBus (`LocalEventBus`) with pub/sub and request-reply |
| `r2e-events-iggy` / `-kafka` / `-pulsar` / `-rabbitmq` | Distributed EventBus backends (`r2e-events/backends/*`) |
| `r2e-scheduler` | Background task scheduling (interval, cron) on the Executor pool |
| `r2e-executor` | Managed task pool: `PoolExecutor`, `#[async_exec]`, `#[derive(BackgroundService)]` |
| `r2e-data-sqlx` | Managed SQLx transactions + `SqlxDataSource` |
| `r2e-data-diesel` | Managed Diesel transactions + `DieselDataSource` |
| `r2e-cache` | TTL cache with pluggable backends |
| `r2e-rate-limit` | Token-bucket rate limiting |
| `r2e-openapi` | OpenAPI 3.1.0 spec generation + docs UI |
| `r2e-prometheus` | HTTP metrics (Prometheus, layer-only, `metrics` facade) |
| `r2e-observability` | OpenTelemetry tracing + context propagation (OTLP) |
| `r2e-grpc` | Tonic gRPC server, separate port or multiplexed; grpc-web |
| `r2e-grpc-build` | build.rs helper compiling `.proto` files (`r2e-grpc/build`) |
| `r2e-mcp` | MCP server: `#[tool]` / `#[resource]` / `#[prompt]`, `McpServer` plugin, OAuth 2.1 auth |
| `r2e-oidc` | Embedded OAuth/JWT issuer |
| `r2e-openfga` | OpenFGA authorization; `r2e-openfga-model` (DSL parser) and `r2e-openfga-macros` (`model!`) |
| `r2e-tenant` | Multi-tenant bean routing: `Tenant<T>`, `Tenanted<T>`, `Tenancy` / `PerTenant` |
| `r2e-static` | Embedded static files / SPA |
| `r2e-utils` | Built-in interceptors: Logged, Timed, Cache, CacheInvalidate |
| `r2e-test` | TestApp, TestJwt, TestSession, WsTestClient for integration testing |
| `r2e-devservices` | Testcontainers dev services: DevPostgres, DevRedis, DevKeycloak |
| `r2e-devtools` | Subsecond hot-reload (`dev-reload` feature) |
| `r2e-cli` | CLI: `r2e new/add/dev/generate/doctor/routes/docs` (not published; `cargo install --path r2e-cli`) |

Test-only members (`publish = false`): `r2e-compile-tests` (trybuild),
`r2e-edition2024-tests`, `r2e-featureless-tests`, `llm-doctests`, and
`r2e-openfga/codegen` (the OpenFGA proto generator).

For a file-by-file breakdown see [`REPO_MAP.md`](REPO_MAP.md); for the book's
crate map see [`docs/book/src/reference/crate-map.md`](docs/book/src/reference/crate-map.md).

## Proc macro development

The macro crate (`r2e-macros/src`) is split by stage:

- `attrs/` — attribute entry points: `controller_attr.rs`, `routes_attr.rs`,
  `bean_attr.rs`, `producer_attr.rs`, `module_attr.rs`, `main_attr.rs`,
  `test_suite_attr.rs`
- `parsing/` — `controller_parsing.rs`, `routes_parsing.rs`,
  `grpc_routes_parsing.rs`, `mcp_routes_parsing.rs`, `member.rs`,
  `identity_param.rs`
- `extract/` — per-method attribute extraction (route, `#[scheduled]`,
  `#[consumer]`, `#[async_exec]`, `#[managed]`, plugins, durations)
- `model/` — the intermediate model (routes, field resolution, type lists)
- `codegen/` — HTTP controller codegen: `controller_codegen.rs`,
  `controller_impl.rs`, `handlers.rs`, `decorators.rs`, `wrapping.rs`,
  `registration.rs`, `scheduled.rs`, `transverse.rs`, `identity_path.rs`,
  `precise_capture.rs`
- `grpc_codegen/`, `mcp_codegen/` — gRPC and MCP adapters
- `derives/` — `#[derive(...)]` macros (`ApiError`, `Bean`, `BackgroundService`,
  `DecoratorBean`, `ConfigProperties`, `Params`, `ProvideBundle`, …)
- `util/` — shared helpers (`crate_path.rs`, `self_method.rs`, `type_utils.rs`, …)

The two main paths are:

- **Controller:** `attrs/controller_attr.rs` → `parsing/controller_parsing.rs` → `codegen/controller_codegen.rs`
- **Routes:** `attrs/routes_attr.rs` → `parsing/routes_parsing.rs` → `codegen/controller_impl.rs` + `codegen/handlers.rs`

Use `cargo expand` to inspect generated code:

```bash
cargo expand -p example-app --lib controllers::user_controller
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
fix(macros): handle unit structs in #[controller]
docs: add macro debugging guide
```

### Before submitting

All of the following must pass (CI runs everything except clippy; see
[`docs/ci.md`](docs/ci.md)):

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Containment boundaries (axum / tokio / JSON codec); baselines only shrink
scripts/check-dep-boundary.sh
scripts/check-source-boundary.sh

# Version invariants (no version change in a feature PR, pins, changelogs)
scripts/check-versions.sh
```

Depending on what you touched:

```bash
# r2e-core: the dev-reload feature is off by default
cargo check -p r2e-core --features dev-reload --all-targets

# Macro error messages
cargo test -p r2e-compile-tests

# Public API surface or llm/: snippets must compile, llm-full.txt must be regenerated
cargo test -p llm-doctests
scripts/check-llm-docs.sh --update

# OpenFGA proto (needs protoc)
scripts/generate-openfga-proto.sh --check
```

Tests live in `<crate>/tests/` (no inline `#[cfg(test)]` modules), grouped into
one Cargo target per subsystem, and async tests use `#[r2e::test]` /
`#[r2e_core::test]`, never `#[tokio::test]` (CI fails on new occurrences).
Format the files you changed with `rustfmt`; avoid `cargo fmt --all`, which
reformats unrelated files under the pinned nightly.

## Versioning and releases

Every publishable crate is versioned independently: it owns its `version`, its
`<crate>/CHANGELOG.md` and its tags `<crate>-vX.Y.Z`. The root `CHANGELOG.md`
is frozen history up to 0.6.0.

- **Feature PRs never change a version.** Record the change under
  `## [Unreleased]` in each affected crate's `CHANGELOG.md`.
- **Breaking changes** go under a `Breaking` entry. A breaking change bumps that
  crate's minor version (pre-1.0) and propagates to every publishable crate that
  depends on it, the facade `r2e` included (`r2e-cli` follows `r2e`). When the
  migration is more than a rename, add a step-by-step guide under
  `docs/migration/`. Prefer a compile-time migration path: the old API fails to
  build with an error naming the migration page.
- **A release is its own `release: …` PR**, made with
  `scripts/bump-version.sh <crate> patch|minor|breaking|X.Y.Z` (`--dry-run` to
  preview). It computes the propagation and rewrites manifests, pins and
  changelogs. `r2e-macros` breaking bumps use `--no-propagate`; its direct
  consumers take a patch bump to re-pin it. A new crate's first release is
  `bump-version.sh <crate> X.Y.Z` at its current version.
- **A release leaves nothing behind**: a crate whose source changed since its
  last tag must be bumped too, or `scripts/check-versions.sh` fails while the
  release is in flight.
- **Publishing happens only in CI** (`.github/workflows/release.yml`): after
  Tests pass on master, it publishes the crates whose version changed, tags
  them, creates the GitHub releases and smoke-builds an app against crates.io
  (`scripts/smoke-registry.sh`). `r2e-cli` is held back from crates.io.


## Good first issues

Look for these labels on the issue tracker:

- **`good-first-issue`** — well-scoped, beginner-friendly tasks
- **`help-wanted`** — contributions welcome, may require more context
- **`docs`** — documentation improvements
- **`proc-macro`** — work in `r2e-macros` (parsing, codegen, error messages)

## Questions?

Open an issue or start a discussion on the repository. We're happy to help!
