//! R2E prelude — import everything you need with a single `use`.
//!
//! ```ignore
//! use r2e_core::prelude::*;
//!
//! #[controller(path = "/my")]
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

/// Attribute macro — emits the physical struct plus its metadata, Axum
/// request façade, extractor, and `ContextConstruct` impl.
pub use r2e_macros::controller;

/// Attribute macro on `impl` blocks — generates Axum handlers, route wiring,
/// and `Controller` trait impl.
pub use r2e_macros::routes;

// HTTP method attributes
pub use r2e_macros::{delete, get, patch, post, put};

// Route-level attributes
pub use r2e_macros::{guard, intercept, layer, managed, middleware, pre_guard, returns, roles, status, transactional};

// SSE & WebSocket attributes
pub use r2e_macros::{sse, ws};

// Event & scheduling attributes
pub use r2e_macros::{consumer, scheduled};

// gRPC attribute
pub use r2e_macros::grpc_routes;

// Entry-point macro (r2e::test is NOT in prelude to avoid conflict with #[test])
pub use r2e_macros::main;

// Bean / DI macros
pub use r2e_macros::bean;
pub use r2e_macros::module;
pub use r2e_macros::producer;
pub use r2e_macros::Bean;
pub use r2e_macros::DecoratorBean;
pub use r2e_macros::BackgroundService;

// Config macros
pub use r2e_macros::ConfigProperties;
pub use r2e_macros::FromConfigValue;

// Cache macros
pub use r2e_macros::Cacheable;

// Error macros
pub use r2e_macros::ApiError;

// ── Core types (from r2e-core) ──────────────────────────────────────────

pub use crate::builder::{AppBuilder, BootableApp, PreparedApp, RegisterController, RegisterControllers, RegisterModule};
// NOTE: `BeanAccess` is deliberately NOT in the prelude: its blanket impl puts
// a `get` method on every type, which would shadow inherent `get`s reached
// through `Deref` (e.g. `Arc<DashMap>::get`). Import it explicitly where
// needed: `use r2e_core::type_list::BeanAccess;`.
pub use crate::type_list::BeanLookup;
pub use crate::controller::ContextConstruct;
pub use crate::decorator::{DecoratorSpec, SelfBuilt};
pub use crate::module::FeatureModule;
pub use crate::extract::{BeanExtract, FromRequestPartsVia, OptionalFromRequestPartsVia, Via};
pub use crate::config::{R2eConfig, ConfigProperties, ConfigValue, ConfigError, ConfigValidationDetail, FromConfigValue, NoChildren, PropertyMeta};
pub use crate::controller::Controller as ControllerTrait;
pub use crate::error::{HttpError, HttpErrorExt};
pub use crate::guards::{
    Guard, GuardContext, GuardError, Identity, NoIdentity, PathParam, PathParams, PreAuthGuard,
    PreAuthGuardContext,
};
pub use crate::interceptors::{Interceptor, InterceptorContext};
pub use crate::managed::{ManagedErr, ManagedResource};
pub use crate::plugin::Plugin;
pub use crate::plugins::{Cors, Tracing, ConfiguredTracing, Health, ErrorHandling, DevReload, NormalizePath, AdvancedHealth};
pub use crate::tracing_config::{LogFormat, SpanEvents, TracingConfig};
pub use crate::request_id::{RequestId, RequestIdPlugin};
pub use crate::secure_headers::SecureHeaders;
pub use crate::event_subscriber::EventSubscriber;

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

// SSE broadcaster + typed topics + per-key rooms
pub use crate::sse::{
    LagPolicy, SseBroadcaster, SseRooms, SseSerializeError, SseSubscription, SseTopic,
};

#[cfg(feature = "multipart")]
pub use crate::multipart::{FromMultipart, Multipart, MultipartSchema, TypedMultipart, UploadedFile};

#[cfg(feature = "multipart")]
pub use r2e_macros::FromMultipart;

#[cfg(feature = "ws")]
pub use crate::http::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};

#[cfg(feature = "ws")]
pub use crate::ws::{WsStream, WsHandler, WsBroadcaster, WsBroadcastReceiver, WsRooms, WsError};

#[cfg(feature = "dev-reload")]
pub use crate::dev::invalidate_state_cache;
