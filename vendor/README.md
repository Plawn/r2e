# Vendored Dependencies

This directory contains patched versions of third-party crates that cannot be
used as-is from crates.io.

## openfga-rs

**Upstream:** <https://github.com/liamwh/openfga-rs> (git `main`, tonic 0.12)
**crates.io:** `openfga-rs = "0.1.0"` (tonic ~0.11)

### Why we vendor

The crates.io release (0.1.0) depends on `tonic ~0.11`, which does **not**
separate the `channel` (client) and `server` features. Enabling `channel`
requires `transport`, which pulls in `server` → `router` → `axum 0.7` →
`axum-core 0.4`. This conflicts with r2e-core's dependency on axum 0.8 →
axum-core 0.5, producing a **dual axum-core** in the dependency tree and
type-incompatible extractors at compile time.

The upstream git `main` branch bumped to `tonic ~0.12`, where `channel` and
`server` are independent features. We vendor that version with tonic's default
features disabled:

```toml
# vendor/openfga-rs/Cargo.toml (patched line)
tonic = { version = "~0.12", default-features = false, features = ["tls", "channel", "codegen", "prost"] }
```

This gives us the gRPC **client** without pulling in axum at all, keeping a
single `axum-core 0.5` across the workspace.

### What changed vs upstream git

Only `Cargo.toml` was modified — tonic's `default-features` set to `false` and
the feature list narrowed to `["tls", "channel", "codegen", "prost"]`.
All other source files are identical to upstream `main`.

### Why not `cargo-patch` / `patch-crate`?

- `cargo-patch` (v0.3.2) fails to compile on modern Rust toolchains due to
  stale internal cargo dependencies.
- The change is not a small fix against the crates.io release — it requires
  bumping tonic 0.11 → 0.12, prost 0.12 → 0.13, prost-wkt 0.5 → 0.6, and
  modifying `build.rs` (API renames). A diff-based patch would be large and
  fragile.
- Vendoring the git version with one Cargo.toml tweak is the simplest,
  zero-extra-tooling solution.

### Updating

When `openfga-rs` publishes a new crates.io release with tonic >= 0.12 and
separated channel/server features, this vendored copy can be removed:

1. Delete `vendor/openfga-rs/`.
2. Remove the `[patch.crates-io]` section from the workspace `Cargo.toml`.
3. Update `r2e-openfga/Cargo.toml` to use the new crates.io version.
4. Run `cargo check --workspace` to verify a single axum-core version.
