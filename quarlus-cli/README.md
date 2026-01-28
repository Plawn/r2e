# Quarlus CLI

Command-line tool for scaffolding and managing Quarlus projects.

## Installation

```bash
cargo install --path quarlus-cli
```

This installs the `quarlus` binary.

## Commands

### `quarlus new <name>`

Create a new Quarlus project with a ready-to-use directory structure.

```bash
quarlus new my-app
```

Generates:

```
my-app/
├── Cargo.toml              # Pre-configured with quarlus-core, quarlus-macros, axum, tokio, etc.
├── application.yaml        # App config (name, port)
└── src/
    ├── main.rs             # Entry point with tracing setup
    └── controllers/
        └── mod.rs
```

### `quarlus generate`

Generate boilerplate code for controllers and services.

#### `quarlus generate controller <Name>`

```bash
quarlus generate controller UserController
```

Creates `src/controllers/user_controller.rs` with a `#[derive(Controller)]` struct and a `#[routes]` impl block. Automatically adds the `pub mod` declaration to `src/controllers/mod.rs`.

#### `quarlus generate service <Name>`

```bash
quarlus generate service UserService
```

Creates `src/user_service.rs` with a `#[derive(Clone)]` struct and a constructor.

Names are converted from PascalCase to snake_case for the file name (e.g. `UserController` becomes `user_controller.rs`).

### `quarlus add <extension>`

Add a Quarlus extension crate to your project's `Cargo.toml`.

```bash
quarlus add security
```

Available extensions:

| Extension     | Crate              | Description                              |
|---------------|--------------------|------------------------------------------|
| `security`    | `quarlus-security` | JWT/OIDC authentication                  |
| `data`        | `quarlus-data`     | Entity, Repository, QueryBuilder (SQLx)  |
| `openapi`     | `quarlus-openapi`  | OpenAPI 3.0 spec generation + Swagger UI |
| `events`      | `quarlus-events`   | In-process typed event bus               |
| `scheduler`   | `quarlus-scheduler`| Background task scheduling (cron, interval) |
| `test`        | `quarlus-test`     | Test helpers (TestApp, TestJwt)          |

### `quarlus dev`

Start the development server with automatic recompilation on source changes.

```bash
quarlus dev
```

Runs `cargo watch -x run -w src/` under the hood. Requires [`cargo-watch`](https://github.com/watchexec/cargo-watch):

```bash
cargo install cargo-watch
```

## Typical workflow

```bash
# Bootstrap a project
quarlus new my-api
cd my-api

# Add extensions you need
quarlus add security
quarlus add data
quarlus add openapi

# Scaffold components
quarlus generate controller UserController
quarlus generate service UserService

# Start developing
quarlus dev
```
