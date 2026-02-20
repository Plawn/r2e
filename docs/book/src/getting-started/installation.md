# Installation

## Prerequisites

- **Rust** (stable, 1.75+) — install via [rustup](https://rustup.rs/)
- **Cargo** — included with Rust

## Adding R2E to a project

Add R2E and its common dependencies to your `Cargo.toml`:

```toml
[dependencies]
r2e = { version = "0.1", features = ["full"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

The `"full"` feature enables all R2E sub-crates (security, events, scheduler, data, cache, rate-limit, openapi, utils). You can also pick individual features:

```toml
r2e = { version = "0.1", features = ["security", "data", "openapi"] }
```

### Available features

| Feature | Description |
|---------|-------------|
| `security` | JWT/OIDC authentication (`r2e-security`) |
| `events` | In-process event bus (`r2e-events`) |
| `scheduler` | Background task scheduling (`r2e-scheduler`) |
| `data` | Data access abstractions (`r2e-data`, `r2e-data-sqlx`) |
| `cache` | TTL cache with pluggable backends (`r2e-cache`) |
| `rate-limit` | Token-bucket rate limiting (`r2e-rate-limit`) |
| `openapi` | OpenAPI 3.0.3 spec generation (`r2e-openapi`) |
| `utils` | Built-in interceptors: Logged, Timed, Cache (`r2e-utils`) |
| `full` | Enables all features above |

## Installing the CLI (optional)

The `r2e` CLI provides scaffolding and development commands:

```bash
cargo install r2e-cli
```

This gives you access to:
- `r2e new <name>` — scaffold a new project
- `r2e dev` — start a development server with hot-reload
- `r2e generate` — generate controllers, services, and CRUD scaffolds
- `r2e doctor` — check your project setup
- `r2e routes` — list all registered routes

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

You should see the server start on `http://localhost:8080`. Visit `http://localhost:8080/` to see "Hello, World!" and `http://localhost:8080/health` for the health check.
