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

// gRPC attribute
pub use r2e_macros::grpc_routes;

// Bean / DI macros
pub use r2e_macros::bean;
pub use r2e_macros::producer;
pub use r2e_macros::Bean;
pub use r2e_macros::BeanState;

// Config macros
pub use r2e_macros::ConfigProperties;

// Cache macros
pub use r2e_macros::Cacheable;

// Error macros
pub use r2e_macros::ApiError;

// ── Core types (from r2e-core) ──────────────────────────────────────────

pub use crate::builder::AppBuilder;
pub use crate::config::{R2eConfig, ConfigProperties, ConfigValue, ConfigError, FromConfigValue};
pub use crate::controller::Controller as ControllerTrait;
pub use crate::error::HttpError;
pub use crate::guards::{Guard, GuardContext, Identity, NoIdentity, PreAuthGuard, PreAuthGuardContext};
pub use crate::interceptors::{Interceptor, InterceptorContext};
pub use crate::managed::{ManagedErr, ManagedError, ManagedResource};
pub use crate::plugin::Plugin;
pub use crate::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload, NormalizePath, AdvancedHealth};
pub use crate::request_id::{RequestId, RequestIdPlugin};
pub use crate::secure_headers::SecureHeaders;
pub use crate::controller::StatefulConstruct;

// ── Type aliases ──────────────────────────────────────────────────────────

pub use crate::types::{ApiResult, JsonResult, StatusResult};

// ── HTTP re-exports ────────────────────────────────────────────────────────

// Core types
pub use crate::http::{Json, Router, StatusCode, HeaderMap, Uri, Extension, Body, Bytes};

// Extractors
pub use crate::http::extract::{
    ConnectInfo, DefaultBodyLimit, Form, FromRef, FromRequest, FromRequestParts,
    MatchedPath, OriginalUri, Path, Query, Request, State,
};

// Headers
pub use crate::http::header::{
    HeaderName, HeaderValue, Method,
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST as HOST_HEADER,
    LOCATION, ORIGIN, REFERER, SET_COOKIE, USER_AGENT,
};

// Response types
pub use crate::http::response::{Html, IntoResponse, Redirect, Response, Sse, SseEvent, SseKeepAlive};

// Middleware
pub use crate::http::middleware::{from_fn, Next};

pub use crate::validation::Validate;
pub use r2e_macros::Params;

// SSE broadcaster
pub use crate::sse::SseBroadcaster;

#[cfg(feature = "multipart")]
pub use crate::multipart::{FromMultipart, Multipart, TypedMultipart, UploadedFile};

#[cfg(feature = "multipart")]
pub use r2e_macros::FromMultipart;

#[cfg(feature = "ws")]
pub use crate::http::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};

#[cfg(feature = "ws")]
pub use crate::ws::{WsStream, WsHandler, WsBroadcaster, WsBroadcastReceiver, WsRooms, WsError};
