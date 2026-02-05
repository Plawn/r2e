//! Quarlus — a Quarkus-like ergonomic layer over Axum.
//!
//! This facade crate re-exports all Quarlus sub-crates through a single
//! dependency with feature flags. Import everything you need with:
//!
//! ```ignore
//! use quarlus::prelude::*;
//! ```
//!
//! # Feature flags
//!
//! | Feature       | Default | Crate                |
//! |---------------|---------|----------------------|
//! | `security`    | **yes** | `quarlus-security`   |
//! | `events`      | **yes** | `quarlus-events`     |
//! | `utils`       | **yes** | `quarlus-utils`      |
//! | `data`        | no      | `quarlus-data`       |
//! | `scheduler`   | no      | `quarlus-scheduler`  |
//! | `cache`       | no      | `quarlus-cache`      |
//! | `rate-limit`  | no      | `quarlus-rate-limit` |
//! | `openapi`     | no      | `quarlus-openapi`    |
//! | `prometheus`  | no      | `quarlus-prometheus` |
//! | `openfga`     | no      | `quarlus-openfga`    |
//! | `validation`  | no      | `quarlus-core/validation` |
//! | `full`        | no      | All of the above     |

// Re-export sub-crates as public modules so they're accessible as
// `quarlus::quarlus_core`, `quarlus::quarlus_events`, etc.
//
// The proc macros use `proc-macro-crate` to detect whether the user depends
// on `quarlus` (facade) or individual crates, and generate the correct paths.
pub extern crate quarlus_core;
pub extern crate quarlus_macros;

#[cfg(feature = "rate-limit")]
pub extern crate quarlus_rate_limit;

// Re-export everything from quarlus-core at the top level for convenience.
pub use quarlus_core::*;

#[cfg(feature = "security")]
pub use quarlus_security;

#[cfg(feature = "events")]
pub use quarlus_events;

#[cfg(feature = "utils")]
pub use quarlus_utils;

#[cfg(feature = "data")]
pub use quarlus_data;

#[cfg(feature = "scheduler")]
pub use quarlus_scheduler;

#[cfg(feature = "cache")]
pub use quarlus_cache;

#[cfg(feature = "openapi")]
pub use quarlus_openapi;

#[cfg(feature = "prometheus")]
pub use quarlus_prometheus;

#[cfg(feature = "openfga")]
pub use quarlus_openfga;

/// Convenience type aliases that depend on types from optional sub-crates.
pub mod types {
    pub use quarlus_core::types::*;

    /// Paginated JSON result — `Result<Json<Page<T>>, AppError>`.
    ///
    /// Available when the `data` feature is enabled.
    ///
    /// ```ignore
    /// #[get("/users")]
    /// async fn list(&self, pageable: Pageable) -> PagedResult<User> {
    ///     Ok(Json(self.service.list(pageable).await?))
    /// }
    /// ```
    #[cfg(feature = "data")]
    pub type PagedResult<T> =
        Result<quarlus_core::http::Json<quarlus_data::Page<T>>, quarlus_core::AppError>;
}

/// Unified prelude — import everything with `use quarlus::prelude::*`.
///
/// Includes the core prelude plus types from all enabled feature crates.
pub mod prelude {
    pub use quarlus_core::prelude::*;
    pub use crate::types::*;

    #[cfg(feature = "security")]
    pub use quarlus_security::prelude::*;

    #[cfg(feature = "data")]
    pub use quarlus_data::prelude::*;

    #[cfg(feature = "events")]
    pub use quarlus_events::prelude::*;

    #[cfg(feature = "utils")]
    pub use quarlus_utils::prelude::*;

    #[cfg(feature = "openfga")]
    pub use quarlus_openfga::prelude::*;
}
