//! MCP (Model Context Protocol) server support for R2E.
//!
//! Exposes `#[tool]` methods on `#[mcp_routes]` types over MCP's streamable
//! HTTP transport, with the same DX as HTTP controllers: `#[inject]`,
//! `#[config]`, guards (`#[roles]`/`#[guard]` — the SAME `Guard<I>` machinery
//! HTTP uses), interceptors, typed config, graceful shutdown, sharded
//! serving.
//!
//! rmcp is the wire only — R2E dispatches tools itself; every rmcp type
//! relevant to users stays behind this crate's own surface.
//!
//! # Example
//!
//! ```ignore
//! use r2e_mcp::prelude::*;
//!
//! #[controller]
//! pub struct MathTools {
//!     #[inject] calc: CalcService,
//! }
//!
//! #[mcp_routes]
//! impl MathTools {
//!     /// Add two numbers.
//!     #[tool(name = "add", read_only)]
//!     async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
//!         Json(self.calc.add(p.a, p.b))
//!     }
//! }
//!
//! AppBuilder::new()
//!     .provide(CalcService::new())
//!     .plugin(McpServer::new())
//!     .build_state()
//!     .await
//!     .register_mcp_service::<MathTools>()
//!     .serve_auto()
//! ```

pub mod auth;
pub mod config;
pub mod error;
pub mod guard;
pub mod handler;
pub mod params;
pub mod plugin;
pub mod registry;
pub mod result;
pub mod route;
pub mod service;

use r2e_core::type_list::AllSatisfied;
use r2e_core::EndpointDeps;

pub use auth::{McpAuthConfig, McpPrincipal, McpTokenValidator, ToolRequirements};
pub use config::McpConfig;
pub use error::McpError;
pub use params::{Params, ToolParams};
pub use plugin::{McpMarker, McpServer};
pub use registry::{McpServiceRegistry, RegisteredMcpService};
pub use result::IntoToolResult;
pub use route::{SchemaObject, ToolAnnotations, ToolCall, ToolFuture, ToolInvoke, ToolRoute};
pub use service::McpService;

/// The wire result type of a tool call (rmcp's `CallToolResult`), re-exported
/// for tools that build results by hand.
pub use rmcp::model::CallToolResult;

/// Re-export of `schemars` for generated code (input/output schema probes)
/// and for deriving `JsonSchema` on tool parameter types without a direct
/// dependency.
pub use schemars;

/// Extension trait for `AppBuilder` to register MCP services.
///
/// The MCP analog of `register_controller` for HTTP — including the
/// compile-time dependency check: the service's [`EndpointDeps`] (its core's
/// `#[inject]` fields plus every `#[intercept(...)]`/`#[guard(...)]` spec's
/// deps, emitted by `#[mcp_routes]`) are checked against the application
/// state via `AllSatisfied`, so a missing bean is a compile error at this
/// call site.
///
/// `T` and `DepIdx` are inference-only witnesses (the same pattern as
/// [`RegisterController`](r2e_core::RegisterController)): call sites write
/// `.register_mcp_service::<MathTools>()` and never name them.
pub trait AppBuilderMcpExt<T, DepIdx>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Register an MCP service whose tools are wired into the MCP endpoint.
    ///
    /// The service is built immediately from the retained bean graph
    /// ([`AppBuilder::bean_context`](r2e_core::AppBuilder::bean_context)).
    ///
    /// # Panics
    ///
    /// Panics if config keys or sections declared on the service (or on any
    /// of its decorator specs) fail validation. Use
    /// [`try_register_mcp_service`](Self::try_register_mcp_service) for a
    /// non-panicking alternative.
    fn register_mcp_service<S>(self) -> Self
    where
        S: McpService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>;

    /// Register an MCP service, returning config-validation errors instead
    /// of panicking.
    ///
    /// Behaves exactly like
    /// [`register_mcp_service`](Self::register_mcp_service) on success. On
    /// failure the service's aggregated
    /// [`ConfigValidationError`](r2e_core::config::ConfigValidationError) is
    /// returned and the builder is consumed.
    fn try_register_mcp_service<S>(self) -> Result<Self, r2e_core::config::ConfigValidationError>
    where
        S: McpService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>;
}

impl<T, DepIdx> AppBuilderMcpExt<T, DepIdx> for r2e_core::AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn register_mcp_service<S>(self) -> Self
    where
        S: McpService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>,
    {
        self.try_register_mcp_service::<S>().unwrap_or_else(|err| {
            panic!(
                "\n=== CONFIGURATION ERRORS (mcp service: {}) ===\n\n{}\n============================\n",
                std::any::type_name::<S>(),
                err
            )
        })
    }

    fn try_register_mcp_service<S>(self) -> Result<Self, r2e_core::config::ConfigValidationError>
    where
        S: McpService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>,
    {
        // Aggregated config validation, before anything is built — the MCP
        // peer of the `register_controller` banner. Covers the core's own
        // `#[config]` keys and every decorator spec's.
        if let Some(config) = self.r2e_config() {
            let errors = S::validate_config(config);
            if !errors.is_empty() {
                return Err(r2e_core::config::ConfigValidationError { errors });
            }
        }

        let registry = self
            .get_plugin_data::<McpServiceRegistry>()
            .expect(
                "McpServiceRegistry not found. Did you install `.plugin(McpServer::new())` before build_state()?",
            )
            .clone();

        registry.add_service(S::service_name(), S::tools(self.bean_context()));

        tracing::debug!(service = S::service_name(), "Registered MCP service");

        Ok(self)
    }
}

/// Re-exports for generated code.
#[doc(hidden)]
pub mod __macro_support {
    pub use crate::error::McpError;
    pub use crate::guard::{guard_response_to_error, tool_guard_context};
    pub use crate::params::{empty_object_schema, schema_object_for, Params, ToolParams};
    pub use crate::result::IntoToolResult;
    pub use crate::route::{
        SchemaObject, ToolAnnotations, ToolCall, ToolFuture, ToolInvoke, ToolRoute,
    };
    pub use crate::service::McpService;
    pub use r2e_core::{ContextConstruct, Guard, Identity, NoIdentity};
    pub use rmcp::model::CallToolResult;
}

pub mod prelude {
    //! Re-exports of the most commonly used MCP types.
    pub use crate::error::McpError;
    pub use crate::params::Params;
    pub use crate::auth::{McpAuthConfig, McpTokenValidator};
    pub use crate::plugin::McpServer;
    pub use crate::route::ToolCall;
    pub use crate::service::McpService;
    pub use crate::AppBuilderMcpExt;
}
