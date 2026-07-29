//! Shared test helpers for the `r2e-data-sqlx` test targets.
//!
//! This file is **not** a test target of its own (no `main.rs` in the
//! directory, so Cargo ignores it). Each target pulls it in with:
//!
//! ```ignore
//! #[path = "../support/mod.rs"]
//! mod support;
//! ```

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use r2e_core::config::LiveConfig;
use r2e_core::{LiveConfigRegistry, R2eConfig};

/// Makes every temp database file unique inside one test binary, so tests that
/// run concurrently never share a SQLite file.
static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// A `sqlite://` URL backed by a fresh temp file.
///
/// File-backed (not `:memory:`) on purpose: rotation tests need two databases
/// that are distinguishable from each other and survive a pool swap.
pub fn sqlite_file_url(label: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "r2e-sqlx-{label}-{}-{}.db",
        process::id(),
        NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    format!("sqlite://{}?mode=rwc", path.display())
}

/// Remove the temp file behind a [`sqlite_file_url`].
pub fn cleanup_sqlite_file(url: &str) {
    let Some(path) = url
        .strip_prefix("sqlite://")
        .and_then(|rest| rest.split_once("?mode=rwc"))
        .map(|(path, _)| PathBuf::from(path))
    else {
        return;
    };
    let _ = std::fs::remove_file(path);
}

/// A live-config handle seeded with `url`, plus the registry that can push a
/// new value to it.
///
/// The returned [`LiveConfig`] keeps its own registry handle alive, so callers
/// only need the registry when they want to publish an update.
pub fn live_url(key: &str, url: &str) -> (LiveConfig<String>, LiveConfigRegistry) {
    let mut config = R2eConfig::empty();
    config.set(key, url.into());
    let registry = LiveConfigRegistry::from_config(&config, HashSet::new());
    (registry.live_config(key), registry)
}
