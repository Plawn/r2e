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
//! Both paths take a spec: [`PostgresSpec`] (image + credentials) via
//! [`start_with`](DevPostgres::start_with) /
//! [`shared_with`](DevPostgres::shared_with), [`RedisImage`] via the same pair
//! on [`DevRedis`]. Everything in the spec is part of the shared container's
//! identity, so two specs that differ get two containers.
//!
//! # Any other service
//!
//! [`DevService`] is the same machinery — labels, Ryuk, cross-process sharing —
//! open to any testcontainers [`Image`](testcontainers::Image), so a service
//! R2E ships no wrapper for is a few lines on your side:
//!
//! ```ignore
//! use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
//! use r2e_devservices::testcontainers::{GenericImage, ImageExt};
//! use r2e_devservices::{DevService, DevServiceSpec};
//!
//! let spec = DevServiceSpec::new("clickhouse", || {
//!     GenericImage::new("clickhouse/clickhouse-server", "24.8-alpine")
//!         .with_exposed_port(8123.tcp())
//!         .with_wait_for(WaitFor::message_on_either_std("Ready for connections"))
//!         .into()
//! })
//! .with_port(8123);
//!
//! let clickhouse = DevService::shared(spec).await;
//! let url = format!("http://{}", clickhouse.endpoint(8123));
//! ```
//!
//! `DevPostgres` and friends are thin wrappers over exactly this — they add a
//! typed URL, nothing more. Two specs share a container when their whole
//! request matches (image, env, command, mounts, …), so nothing has to be
//! declared for a different image or different credentials to get a container
//! of their own.
//!
//! Feature flags: `postgres`, `redis`, `openfga`.

mod common;
mod ryuk;
mod service;

pub use service::{DevService, DevServiceSpec};

/// Re-exported so a user-defined [`DevServiceSpec`] builds its image against
/// the exact same `testcontainers` version — a mismatched one yields a
/// different `ContainerRequest` type and will not compile.
pub use testcontainers;
/// Re-exported for the ready-made images (`clickhouse`, `mongo`, `kafka`, …).
///
/// Each image sits behind its own feature on `testcontainers-modules`; add the
/// crate to your own `[dev-dependencies]` with the feature you need and Cargo
/// unifies it with this re-export.
pub use testcontainers_modules;

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "postgres")]
pub use postgres::{DevPostgres, PostgresImage, PostgresSpec};

#[cfg(feature = "redis")]
mod redis;
#[cfg(feature = "redis")]
pub use redis::{DevRedis, RedisImage};

#[cfg(feature = "openfga")]
mod openfga;
#[cfg(feature = "openfga")]
pub use openfga::DevOpenFga;
