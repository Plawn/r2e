//! Header and request-line types.
//!
//! These are **`http` crate types**, not axum types — axum merely re-exports
//! them from `http`, and the workspace resolves a single `http` version, so
//! sourcing them here is type-identical to sourcing them from axum's own
//! re-export (plans/runtime-http-dependency-containment.md §5 step 3a). Doing
//! it this way means the largest share of "axum names" in R2E is not an axum
//! coupling at all, and what the boundary check still counts is an honest
//! measure of the real HTTP-layer coupling.

// Re-export the entire http::header module for access to all constants
pub use http::header::*;
pub use http::request::Parts;
pub use http::{HeaderMap, Method, Request as HttpRequest, StatusCode};
