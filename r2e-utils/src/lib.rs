//! Built-in interceptors for R2E controllers and beans.
//!
//! Enabled in the `r2e` facade by feature `utils` (re-exported as
//! `r2e::r2e_utils`; the interceptors are also in `r2e::prelude`). Each type is
//! an `Interceptor` applied with `#[intercept(..)]`:
//!
//! - [`Logged`] — logs entry and exit at a [`LogLevel`] (`Logged::info()`,
//!   `Logged::debug()`, …).
//! - [`Timed`] — logs the execution time, optionally only above a threshold
//!   (`Timed::threshold(100)`).
//! - [`Cache`] — caches `Json<T>` responses in the `CacheStore` bean
//!   (`Cache::ttl(30)`, named groups with `.group("users")`);
//!   [`CacheInvalidate`] clears a group after the method runs.
//! - [`Counted`] / [`MetricTimed`] — a named invocation counter and a named
//!   duration metric, both emitted through `tracing`.
//!
//! Interceptors stack in declaration order (outermost first):
//!
//! ```rust,ignore
//! use r2e::prelude::*;
//!
//! #[routes]
//! #[intercept(Logged::info())]
//! impl UserController {
//!     #[get("/")]
//!     #[intercept(Timed::threshold(100))]
//!     #[intercept(Cache::ttl(30).group("users"))]
//!     async fn list(&self) -> Json<Vec<User>> {
//!         Json(self.user_service.list().await)
//!     }
//!
//!     #[post("/")]
//!     #[intercept(CacheInvalidate::group("users"))]
//!     async fn create(&self, Json(body): Json<CreateUser>) -> Json<User> {
//!         Json(self.user_service.create(body).await)
//!     }
//! }
//! ```
//!
//! # Guides
//!
//! [`docs/claude/guards-interceptors.md`](https://github.com/plawn/r2e/blob/master/docs/claude/guards-interceptors.md)
//! and the crate README.

pub mod interceptors;
pub use interceptors::{
    log_at_level, Cache, CacheInvalidate, Counted, LogLevel, Logged, MetricTimed, Timed,
};

pub mod prelude {
    //! Re-exports of the most commonly used utility interceptors.
    pub use crate::interceptors::{Cache, CacheInvalidate, Counted, Logged, MetricTimed, Timed};
}
