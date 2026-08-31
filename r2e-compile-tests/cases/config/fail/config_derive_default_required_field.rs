//! `#[config(derive_default)]` emits exactly what `from_config` produces
//! against an empty config. A required field (no `#[config(default = ...)]`,
//! not `Option<T>`) has no such value — `from_config` would fail — so the
//! derive refuses rather than inventing `Default::default()` and letting the
//! `Default` silently disagree with config loading.

use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
#[config(derive_default)]
pub struct Settings {
    #[config(default = 8080)]
    pub port: u16,
    pub url: String,
}

fn main() {}
