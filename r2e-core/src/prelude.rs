//! R2E prelude — import everything you need with a single `use`.
//!
//! ```ignore
//! use r2e_core::prelude::*;
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

// ── Macros (from r2e-macros) ────────────────────────────────────────────

/// Derive macro — generates struct metadata, Axum extractor, and
/// `StatefulConstruct` impl (when no `#[inject(identity)]` fields).
pub use r2e_macros::Controller;

/// Attribute macro on `impl` blocks — generates Axum handlers, route wiring,
/// and `Controller` trait impl.
pub use r2e_macros::routes;

// HTTP method attributes
pub use r2e_macros::{delete, get, patch, post, put};

// Route-level attributes
pub use r2e_macros::{guard, intercept, layer, managed, middleware, pre_guard, roles, transactional};

// SSE & WebSocket attributes
pub use r2e_macros::{sse, ws};

// Event & scheduling attributes
pub use r2e_macros::{consumer, scheduled};

// Bean / DI macros
pub use r2e_macros::bean;
pub use r2e_macros::producer;
pub use r2e_macros::Bean;
pub use r2e_macros::BeanState;

// Config macros
pub use r2e_macros::ConfigProperties;

// ── Core types (from r2e-core) ──────────────────────────────────────────

pub use crate::builder::AppBuilder;
pub use crate::config::{R2eConfig, ConfigProperties, ConfigValue, ConfigError, FromConfigValue};
pub use crate::controller::Controller as ControllerTrait;
pub use crate::error::AppError;
pub use crate::interceptors::{Interceptor, InterceptorContext};
pub use crate::managed::{ManagedErr, ManagedError, ManagedResource};
pub use crate::plugin::Plugin;
pub use crate::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload, NormalizePath, AdvancedHealth};
pub use crate::request_id::{RequestId, RequestIdPlugin};
pub use crate::secure_headers::SecureHeaders;

// ── Type aliases ──────────────────────────────────────────────────────────

pub use crate::types::{ApiResult, JsonResult, StatusResult};

// ── HTTP re-exports ────────────────────────────────────────────────────────

pub use crate::http::{Json, Router, StatusCode, HeaderMap};
pub use crate::http::extract::{Path, Query, FromRef, State, Form};
pub use crate::http::response::{IntoResponse, Redirect, Response};

#[cfg(feature = "validation")]
pub use crate::validation::Validated;

#[cfg(feature = "multipart")]
pub use crate::multipart::{FromMultipart, TypedMultipart, UploadedFile};

#[cfg(feature = "multipart")]
pub use r2e_macros::FromMultipart;
