# Step 0 — Cargo Workspace Setup

## Goal

Transform the project into a multi-crate **Cargo workspace** with the target structure.

## Final Structure

```
r2e/
  Cargo.toml              # workspace root
  r2e-core/
    Cargo.toml
    src/lib.rs
  r2e-macros/
    Cargo.toml
    src/lib.rs
  r2e-security/
    Cargo.toml
    src/lib.rs
  example-app/
    Cargo.toml
    src/main.rs
```

## Tasks

### 1. Convert the root `Cargo.toml` into a workspace

```toml
[workspace]
members = [
    "r2e-core",
    "r2e-macros",
    "r2e-security",
    "example-app",
]
resolver = "2"
```

Remove the existing root `src/main.rs` (the application code will go into `example-app`).

### 2. Create `r2e-macros`

```toml
[package]
name = "r2e-macros"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
syn = { version = "2", features = ["full", "extra-traits"] }
quote = "1"
proc-macro2 = "1"
```

`src/lib.rs`: empty file with `extern crate proc_macro;`

### 3. Create `r2e-core`

```toml
[package]
name = "r2e-core"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
r2e-macros = { path = "../r2e-macros" }
```

### 4. Create `r2e-security`

```toml
[package]
name = "r2e-security"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
jsonwebtoken = "9"
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["sync"] }
r2e-core = { path = "../r2e-core" }
```

### 5. Create `example-app`

```toml
[package]
name = "example-app"
version = "0.1.0"
edition = "2021"

[dependencies]
r2e-core = { path = "../r2e-core" }
r2e-macros = { path = "../r2e-macros" }
r2e-security = { path = "../r2e-security" }
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Validation Criteria

```bash
cargo check --workspace
```

Must compile without errors (empty but valid crates).

## Dependencies Between Steps

None — this is the initial step.
