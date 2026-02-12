//! R2E — a Quarkus-like ergonomic layer over Axum.
//!
//! This facade crate re-exports all R2E sub-crates through a single
//! dependency with feature flags. Import everything you need with:
//!
//! ```ignore
//! use r2e::prelude::*;
//! ```
//!
//! # Feature flags
//!
//! | Feature       | Default | Crate                     |
//! |---------------|---------|---------------------------|
//! | `security`    | **yes** | `r2e-security`            |
//! | `events`      | **yes** | `r2e-events`              |
//! | `utils`       | **yes** | `r2e-utils`               |
//! | `data`        | no      | `r2e-data` (abstractions) |
//! | `data-sqlx`   | no      | `r2e-data-sqlx`           |
//! | `data-diesel` | no      | `r2e-data-diesel`         |
//! | `sqlite`      | no      | `r2e-data-sqlx/sqlite`    |
//! | `postgres`    | no      | `r2e-data-sqlx/postgres`  |
//! | `mysql`       | no      | `r2e-data-sqlx/mysql`     |
//! | `scheduler`   | no      | `r2e-scheduler`           |
//! | `cache`       | no      | `r2e-cache`               |
//! | `rate-limit`  | no      | `r2e-rate-limit`          |
//! | `openapi`     | no      | `r2e-openapi`             |
//! | `prometheus`  | no      | `r2e-prometheus`          |
//! | `openfga`     | no      | `r2e-openfga`             |
//! | `validation`  | no      | `r2e-core/validation`     |
//! | `full`        | no      | All of the above          |

// Re-export sub-crates as public modules so they're accessible as
// `r2e::r2e_core`, `r2e::r2e_events`, etc.
//
// The proc macros use `proc-macro-crate` to detect whether the user depends
// on `r2e` (facade) or individual crates, and generate the correct paths.
pub extern crate r2e_core;
pub extern crate r2e_macros;

#[cfg(feature = "rate-limit")]
pub extern crate r2e_rate_limit;

// Re-export everything from r2e-core at the top level for convenience.
pub use r2e_core::*;

#[cfg(feature = "security")]
pub use r2e_security;

#[cfg(feature = "events")]
pub use r2e_events;

#[cfg(feature = "utils")]
pub use r2e_utils;

#[cfg(feature = "data")]
pub use r2e_data;

#[cfg(feature = "data-sqlx")]
pub use r2e_data_sqlx;

#[cfg(feature = "data-diesel")]
pub use r2e_data_diesel;

#[cfg(feature = "scheduler")]
pub use r2e_scheduler;

#[cfg(feature = "cache")]
pub use r2e_cache;

#[cfg(feature = "openapi")]
pub use r2e_openapi;

#[cfg(feature = "prometheus")]
pub use r2e_prometheus;

#[cfg(feature = "openfga")]
pub use r2e_openfga;

#[cfg(feature = "observability")]
pub use r2e_observability;

/// Convenience type aliases that depend on types from optional sub-crates.
pub mod types {
    pub use r2e_core::types::*;

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
        Result<r2e_core::http::Json<r2e_data::Page<T>>, r2e_core::AppError>;
}

/// Unified prelude — import everything with `use r2e::prelude::*`.
///
/// Includes the core prelude plus types from all enabled feature crates.
pub mod prelude {
    pub use r2e_core::prelude::*;
    pub use crate::types::*;

    #[cfg(feature = "security")]
    pub use r2e_security::prelude::*;

    #[cfg(feature = "data")]
    pub use r2e_data::prelude::*;

    #[cfg(feature = "data-sqlx")]
    pub use r2e_data_sqlx::prelude::*;

    #[cfg(feature = "data-diesel")]
    pub use r2e_data_diesel::prelude::*;

    #[cfg(feature = "events")]
    pub use r2e_events::prelude::*;

    #[cfg(feature = "utils")]
    pub use r2e_utils::prelude::*;

    #[cfg(feature = "openfga")]
    pub use r2e_openfga::prelude::*;
}
