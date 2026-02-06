# Etape 0 — Setup du workspace Cargo

## Objectif

Transformer le projet en **workspace Cargo** multi-crates avec la structure cible.

## Structure finale

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

## Taches

### 1. Convertir le `Cargo.toml` racine en workspace

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

Supprimer le `src/main.rs` racine existant (le code applicatif ira dans `example-app`).

### 2. Creer `r2e-macros`

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

`src/lib.rs` : fichier vide avec `extern crate proc_macro;`

### 3. Creer `r2e-core`

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

### 4. Creer `r2e-security`

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

### 5. Creer `example-app`

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

## Critere de validation

```bash
cargo check --workspace
```

Doit compiler sans erreur (crates vides mais valides).

## Dependances entre etapes

Aucune — c'est l'etape initiale.
