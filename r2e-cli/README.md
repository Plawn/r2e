# R2E CLI

Command-line tool for scaffolding and managing R2E projects.

## Installation

```bash
cargo install --path r2e-cli
```

This installs the `r2e` binary.

## Commands

### `r2e new <name>`

Create a new R2E project with a ready-to-use directory structure.

```bash
r2e new my-app
```

Generates:

```
my-app/
├── Cargo.toml              # Pre-configured with r2e-core, r2e-macros, axum, tokio, etc.
├── application.yaml        # App config (name, port)
└── src/
    ├── main.rs             # Entry point with tracing setup
    └── controllers/
        └── mod.rs
```

### `r2e generate`

Generate boilerplate code for controllers and services.

#### `r2e generate controller <Name>`

```bash
r2e generate controller UserController
```

Creates `src/controllers/user_controller.rs` with a `#[derive(Controller)]` struct and a `#[routes]` impl block. Automatically adds the `pub mod` declaration to `src/controllers/mod.rs`.

#### `r2e generate service <Name>`

```bash
r2e generate service UserService
```

Creates `src/user_service.rs` with a `#[derive(Clone)]` struct and a constructor.

Names are converted from PascalCase to snake_case for the file name (e.g. `UserController` becomes `user_controller.rs`).

### `r2e add <extension>`

Add a R2E extension crate to your project's `Cargo.toml`.

```bash
r2e add security
```

Available extensions:

| Extension     | Crate              | Description                              |
|---------------|--------------------|------------------------------------------|
| `security`    | `r2e-security` | JWT/OIDC authentication                  |
| `data`        | `r2e-data`     | Entity, Repository, QueryBuilder (SQLx)  |
| `openapi`     | `r2e-openapi`  | OpenAPI 3.0 spec generation + Swagger UI |
| `events`      | `r2e-events`   | In-process typed event bus               |
| `scheduler`   | `r2e-scheduler`| Background task scheduling (cron, interval) |
| `test`        | `r2e-test`     | Test helpers (TestApp, TestJwt)          |

### `r2e dev`

Start the development server with automatic recompilation on source changes.

```bash
r2e dev
```

Runs `cargo watch -x run -w src/` under the hood. Requires [`cargo-watch`](https://github.com/watchexec/cargo-watch):

```bash
cargo install cargo-watch
```

## Typical workflow

```bash
# Bootstrap a project
r2e new my-api
cd my-api

# Add extensions you need
r2e add security
r2e add data
r2e add openapi

# Scaffold components
r2e generate controller UserController
r2e generate service UserService

# Start developing
r2e dev
```
