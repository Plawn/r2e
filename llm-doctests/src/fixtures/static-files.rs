//! Scaffolding for `llm/static-files.md`.

use r2e::r2e_static::rust_embed;

/// The `rust_embed` asset type the plugin is parameterised over — in an app
/// it points at the frontend build output (`#[folder = "dist/"]`).
#[derive(rust_embed::Embed)]
#[folder = "migrations"]
pub struct Assets;
