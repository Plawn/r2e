//! Dev services for R2E tests — Quarkus-style containerized infrastructure.
//!
//! Each dev service starts a Docker container (via testcontainers) and
//! exposes the connection URL to wire into the test app's config:
//!
//! ```ignore
//! use r2e_devservices::DevPostgres;
//! use r2e_test::TestApp;
//!
//! #[tokio::test]
//! async fn users_are_persisted() {
//!     let pg = DevPostgres::shared().await;
//!     let app = TestApp::boot_with(my_app::app, |b| {
//!         b.override_config_value("app.database.url", pg.url())
//!     })
//!     .await;
//!     // ...
//! }
//! ```
//!
//! [`shared()`](DevPostgres::shared) reuses one stable container across all test
//! processes in the suite. Every process keeps a TCP lease to a shared Ryuk
//! reaper; after the last process exits, Ryuk removes all managed containers.
//! Set `R2E_DEVSERVICES_KEEP=1` to disable reaping for post-mortem inspection.
//! [`start()`](DevPostgres::start) gives an isolated container whose normal
//! lifetime follows the returned handle, with Ryuk as a crash-safe fallback.
//!
//! Feature flags: `postgres`, `redis`, `openfga`.

#[cfg(any(feature = "postgres", feature = "redis", feature = "openfga"))]
mod common;
#[cfg(any(feature = "postgres", feature = "redis", feature = "openfga"))]
mod ryuk;

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "postgres")]
pub use postgres::DevPostgres;

#[cfg(feature = "redis")]
mod redis;
#[cfg(feature = "redis")]
pub use redis::DevRedis;

#[cfg(feature = "openfga")]
mod openfga;
#[cfg(feature = "openfga")]
pub use openfga::DevOpenFga;
