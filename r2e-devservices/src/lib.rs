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
//! [`shared()`](DevPostgres::shared) starts the container **once per test
//! process** and keeps it alive until the process exits (testcontainers'
//! reaper removes it afterwards); [`start()`](DevPostgres::start) gives an
//! isolated container whose lifetime follows the returned handle.
//!
//! Feature flags: `postgres`, `redis`.

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "postgres")]
pub use postgres::DevPostgres;

#[cfg(feature = "redis")]
mod redis;
#[cfg(feature = "redis")]
pub use redis::DevRedis;
