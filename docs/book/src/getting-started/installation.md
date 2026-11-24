# Installation

## Prerequisites

- **Rust** (stable, 1.94+; R2E crates use edition 2024) — install via [rustup](https://rustup.rs/)
- **Cargo** — included with Rust

## Adding R2E to a project

Add R2E and its common dependencies to your `Cargo.toml`:

```toml
[dependencies]
r2e = { version = "0.15", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

The `"full"` feature enables the common sub-crates: security, events, utils, scheduler, executor, cache, rate-limit, openapi, oidc, prometheus, observability, ws, multipart, grpc (with reflection and gRPC-Web), static, tenant and mcp. Database backends, distributed event-bus backends, OpenFGA, QUIC and the dev/test-only features are **not** in `full` and must be added explicitly. You can also pick individual features:

```toml
r2e = { version = "0.15", features = ["security", "sqlx-postgres", "openapi"] }
```

### Available features

The default features are `security`, `events` and `utils`.

| Feature | Description | In `full` |
|---------|-------------|-----------|
| `security` | JWT/OIDC authentication (`r2e-security`) | yes |
| `events` | In-process event bus (`r2e-events`) | yes |
| `utils` | Built-in interceptors: Logged, Timed, Cache (`r2e-utils`) | yes |
| `scheduler` | Background task scheduling (`r2e-scheduler`; pulls in `executor`) | yes |
| `executor` | Managed task pool and background services (`r2e-executor`) | yes |
| `cache` | TTL cache with pluggable backends (`r2e-cache`) | yes |
| `rate-limit` | Token-bucket rate limiting (`r2e-rate-limit`) | yes |
| `openapi` | OpenAPI 3.1.0 spec generation (`r2e-openapi`) | yes |
| `oidc` | Embedded OAuth/JWT issuer (`r2e-oidc`) | yes |
| `prometheus` | HTTP metrics and `/metrics` endpoint (`r2e-prometheus`) | yes |
| `observability` | OpenTelemetry tracing over OTLP (`r2e-observability`) | yes |
| `ws` | WebSockets | yes |
| `multipart` | Multipart form uploads | yes |
| `grpc`, `grpc-reflection`, `grpc-web` | gRPC services, reflection, gRPC-Web (`r2e-grpc`) | yes |
| `static` | Embedded static file serving with SPA support (`r2e-static`) | yes |
| `tenant` | Multi-tenant bean routing (`r2e-tenant`) | yes |
| `mcp` | MCP (Model Context Protocol) server (`r2e-mcp`) | yes |
| `sqlx-{sqlite,postgres,mysql}` | Managed SQLx transactions and the selected driver | no |
| `diesel-{sqlite,postgres,mysql}` | Managed Diesel transactions and the selected driver | no |
| `tenant-sqlx`, `tenant-diesel` | Per-tenant database pools and transactions | no |
| `events-{iggy,kafka,pulsar,rabbitmq}` | Distributed event-bus backends | no |
| `openfga` | OpenFGA fine-grained authorization (`r2e-openfga`) | no |
| `metrics-facade` | HTTP metrics through the `metrics` crate instead of `prometheus` | no |
| `mcp-loopback` | In-process MCP sessions | no |
| `json-sonic` | SIMD JSON codec (sonic-rs) | no |
| `quic` | HTTP/3 over QUIC | no |
| `mcp-testing`, `scheduler-testing`, `test-util` | Test helpers; enable from `[dev-dependencies]` | no |
| `dev-reload` | Hot-reload for `r2e dev`; never in production builds | no |

The `data` feature is a no-op compatibility marker (pagination is always available in core); you do not need it. See the [Crate Map](../reference/crate-map.md#feature-flags) for the exact sub-crates each feature enables.

## Installing the CLI (optional)

The `r2e` CLI provides scaffolding and development commands:

```bash
cargo install --git https://github.com/plawn/r2e r2e-cli
# or, from a checkout of the repository:
cargo install --path r2e-cli
```

The CLI is not published to crates.io (it bundles docs from outside its crate), so install it from the repository.

This gives you access to:
- `r2e new <name>` — scaffold a new project
- `r2e dev` — start a development server with hot-reload
- `r2e generate` — generate controllers, services, and CRUD scaffolds
- `r2e add <extension>` — add an R2E extension (e.g. `security`, `openapi`, `mcp`) to `Cargo.toml`
- `r2e test` — run tests, optionally with `--coverage` or SonarQube LCOV output
- `r2e doctor` — check your project setup
- `r2e routes` — list all registered routes
- `r2e docs` — print bundled, version-matched module docs (`--llm` for the agent-facing reference, `--llm --export` to write it into your project)

## Quick verification

After installing, verify everything works:

```bash
# Create a new project
r2e new hello-r2e

# Enter the project
cd hello-r2e

# Run the app
cargo run
```

You should see the server start on `http://localhost:3000` (the scaffolded `application.yaml` sets `server.port: 3000`, which is also the default when unset). Visit `http://localhost:3000/` to see "Hello, World!" and `http://localhost:3000/health` for the health check.
