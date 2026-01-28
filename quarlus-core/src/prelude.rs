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
//!     async fn hello(&self) -> Json<String> {
//!         Json(self.greeting.clone())
//!     }
//! }
//! ```

// ── Macros (from quarlus-macros) ────────────────────────────────────────────

/// Derive macro — generates struct metadata, Axum extractor, and
/// `StatefulConstruct` impl (when no `#[inject(identity)]` fields).
pub use quarlus_macros::Controller;

/// Attribute macro on `impl` blocks — generates Axum handlers, route wiring,
/// and `Controller` trait impl.
pub use quarlus_macros::routes;

// HTTP method attributes
pub use quarlus_macros::{delete, get, patch, post, put};

// Route-level attributes
pub use quarlus_macros::{guard, intercept, layer, middleware, rate_limited, roles, transactional};

// Event & scheduling attributes
pub use quarlus_macros::{consumer, scheduled};

// Bean / DI macros
pub use quarlus_macros::bean;
pub use quarlus_macros::Bean;
pub use quarlus_macros::BeanState;

// ── Core types (from quarlus-core) ──────────────────────────────────────────

pub use crate::builder::AppBuilder;
pub use crate::controller::Controller as ControllerTrait;
pub use crate::error::AppError;
pub use crate::interceptors::{Interceptor, InterceptorContext};
pub use crate::plugin::Plugin;
pub use crate::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload, NormalizePath};
pub use crate::scheduling::{ScheduleConfig, ScheduledResult, ScheduledTaskDef};

// ── HTTP re-exports ────────────────────────────────────────────────────────

pub use crate::http::{Json, Router, StatusCode, HeaderMap};
pub use crate::http::extract::{Path, Query, FromRef, State};
pub use crate::http::response::{IntoResponse, Response};

#[cfg(feature = "validation")]
pub use crate::validation::Validated;
