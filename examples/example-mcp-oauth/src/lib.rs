//! example-mcp-oauth library.
//!
//! The canonical source lives in `app.rs`, included here so the app can be
//! booted by type (`#[r2e::test(app = McpOAuthApp)]`) while `app_main!`
//! compiles the same file into the binary tip crate.

include!("app.rs");
