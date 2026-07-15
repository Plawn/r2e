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
//! [`shared()`](DevPostgres::shared) reuses a single, stable-named container
//! (`ReuseDirective::Always`) across **every test process and run** — so a
//! suite of many test binaries reuses one warm container instead of spawning
//! (and leaking) one per binary. It is intentionally not removed on process
//! exit. [`start()`](DevPostgres::start) gives an isolated container that
//! testcontainers removes when the returned handle drops; set
//! `R2E_DEVSERVICES_KEEP=1` to keep such one-off containers alive for
//! inspection.
//!
//! Feature flags: `postgres`, `redis`.

#[cfg(any(feature = "postgres", feature = "redis"))]
mod common;

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "postgres")]
pub use postgres::DevPostgres;

#[cfg(feature = "redis")]
mod redis;
#[cfg(feature = "redis")]
pub use redis::DevRedis;
