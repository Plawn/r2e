//! Quarlus prelude — import everything you need with a single `use`.
//!
//! ```ignore
//! use quarlus_core::prelude::*;
//!
//! #[derive(Controller)]
//! #[controller(state = MyState)]
//! pub struct MyController {
//!     #[inject]  my_service: MyService,
//!     #[inject(identity)] user: AuthenticatedUser,
//!     #[config("app.greeting")] greeting: String,
//! }
//!
//! #[routes]
//! impl MyController {
//!     #[get("/hello")]
//!     async fn hello(&self) -> axum::Json<String> {
//!         axum::Json(self.greeting.clone())
//!     }
//! }
//! ```

// ── Macros (from quarlus-macros) ────────────────────────────────────────────

/// Derive macro — generates struct metadata, Axum extractor, and
/// `StatefulConstruct` impl (when no `#[inject(identity)]` fields).
pub use quarlus_macros::Controller;

/// Attribute macro on `impl` blocks — generates Axum handlers, route wiring,
/// and `Controller` / `ScheduledController` trait impls.
pub use quarlus_macros::routes;

// HTTP method attributes
pub use quarlus_macros::{delete, get, patch, post, put};

// Route-level attributes
pub use quarlus_macros::{guard, intercept, middleware, rate_limited, roles, transactional};

// Event & scheduling attributes
pub use quarlus_macros::{consumer, scheduled};

// ── Core types (from quarlus-core) ──────────────────────────────────────────

pub use crate::builder::AppBuilder;
pub use crate::controller::Controller as ControllerTrait;
pub use crate::error::AppError;
pub use crate::interceptors::{Interceptor, InterceptorContext};

#[cfg(feature = "validation")]
pub use crate::validation::Validated;
