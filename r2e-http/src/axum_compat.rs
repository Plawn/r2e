//! Escape hatch: the raw `axum` API, reachable on purpose.
//!
//! **Importing from here couples your application to the axum backend.** The
//! supported public surface of R2E is the R2E types under `r2e::http` (and the
//! subset of them in `r2e::prelude`); R2E's own contracts —
//! `r2e::web::extract::FromRequestPartsVia` for extraction,
//! [`IntoHttpResponse`](crate::response::IntoHttpResponse) for responses — are
//! what R2E promises to keep working across an HTTP-backend change. Nothing
//! here is covered by that promise.
//!
//! It exists because a re-export shim can never be complete: tower layers with
//! axum-typed bounds, `axum::debug_handler`, a third-party crate whose API is
//! spelled in axum types, an extractor R2E has not re-exported. Reaching for
//! axum directly is a legitimate answer to those — the point of this module is
//! that it be **explicit and greppable** (`rg axum_compat`) rather than a
//! `Cargo.toml` line nobody revisits.
//!
//! ```ignore
//! // the whole crate, under its own name
//! use r2e::http::axum_compat::axum;
//!
//! // or an item directly
//! use r2e::http::axum_compat::routing::method_routing;
//! ```
//!
//! If what you need turns out to be broadly useful, prefer opening an issue to
//! have it re-exported from `r2e::http` over spreading `axum_compat` imports.
//!
//! See `plans/runtime-http-dependency-containment.md` §5.3d (decision A,
//! 2026-08-24).

/// The `axum` crate itself, under its own name.
pub use axum;

pub use axum::*;
