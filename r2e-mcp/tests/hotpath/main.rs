//! Per-request allocation guards for the MCP hot paths (tasks #994 / #993,
//! `docs/claude/hot-path-clone-audit.md`).
//!
//! Two invariants, neither of them observable from a behavioural test — the
//! responses are identical either way, only the allocator sees the
//! difference:
//!
//! * `*/list` clones the prebuilt wire payload; the macro-emitted metadata in
//!   it must be borrowed, so the clone copies pointers, not strings (#994).
//! * an authenticated request builds ONE `AuthenticatedUser` and shares it
//!   between `McpPrincipal` and the identity extension (#993).
//!
//! The global allocator counts per thread, so every measured future is driven
//! on a `current_thread` runtime (`counter::runtime()`).

#[global_allocator]
static GLOBAL: counter::CountingAllocator = counter::CountingAllocator;

#[path = "../support/mod.rs"]
mod support;

mod counter;

mod lists;
mod principal;
