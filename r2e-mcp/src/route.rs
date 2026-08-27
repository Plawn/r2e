//! Tool routes: the unit `#[mcp_routes]` compiles each `#[tool]` method into.
//!
//! A [`ToolRoute`] carries the tool's wire metadata (name, description,
//! schemas, annotations) plus a type-erased [`ToolInvoke`] closure that owns
//! the service wrapper and runs the actual method. The handler
//! ([`crate::handler`]) precomputes the rmcp `Tool` list from the metadata and
//! dispatches `tools/call` to the closure.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use r2e_core::http::Parts;
use r2e_core::rt::CancelToken;
use rmcp::model::CallToolResult;
use serde_json::Value;

use crate::error::McpError;

/// A JSON Schema object body (the map under `inputSchema`).
pub type SchemaObject = serde_json::Map<String, Value>;

/// Everything a tool invocation can observe about its call.
///
/// Handed to the generated dispatch closure; tool methods can also take it as
/// a parameter to read raw arguments, HTTP request parts, or the cancellation
/// token.
#[derive(Clone)]
pub struct ToolCall {
    /// The raw `arguments` object of the `tools/call` request (an empty JSON
    /// object when the client sent none).
    pub arguments: Value,
    /// The HTTP request parts of the transport request carrying this call
    /// (headers, URI, extensions). `None` only when the transport did not
    /// provide them (e.g. hand-built calls in tests).
    ///
    /// Auth layers insert the caller's identity into `parts.extensions`;
    /// `#[inject(identity)]` tool parameters are resolved from there.
    pub parts: Option<Arc<Parts>>,
    /// The JSON-RPC request id, stringified (for logging/correlation).
    pub request_id: String,
    /// Cancelled when the client aborts the request or the server shuts
    /// down. Long-running tools should observe it.
    pub cancel: CancelToken,
}

impl ToolCall {
    /// Build a bare call for tests: `arguments` only, no HTTP parts, a fresh
    /// cancellation token.
    pub fn for_test(arguments: Value) -> Self {
        ToolCall {
            arguments,
            parts: None,
            request_id: "test".to_string(),
            cancel: CancelToken::new(),
        }
    }

    /// Read a request-scoped value of type `T` from the HTTP request
    /// extensions (where auth layers deposit the caller's identity).
    ///
    /// `None` when there are no parts or no `T` was inserted.
    pub fn extension<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.parts.as_ref()?.extensions.get::<T>().cloned()
    }
}

/// Boxed future returned by a tool invocation.
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send>>;

/// Type-erased tool invocation closure. Owns (an `Arc` of) the service
/// wrapper; called once per `tools/call`.
pub type ToolInvoke = Arc<dyn Fn(ToolCall) -> ToolFuture + Send + Sync>;

/// Behavioral hints advertised with a tool (MCP `ToolAnnotations`).
///
/// All hints are advisory — clients use them for display and confirmation
/// UX, never for security decisions.
#[derive(Debug, Clone, Default)]
pub struct ToolAnnotations {
    /// Human-readable title.
    pub title: Option<String>,
    /// The tool does not modify its environment.
    pub read_only: Option<bool>,
    /// The tool may perform destructive updates (meaningful when not
    /// read-only).
    pub destructive: Option<bool>,
    /// Repeated calls with the same arguments have no additional effect.
    pub idempotent: Option<bool>,
    /// The tool interacts with an open world of external entities.
    pub open_world: Option<bool>,
}

impl ToolAnnotations {
    pub(crate) fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.read_only.is_none()
            && self.destructive.is_none()
            && self.idempotent.is_none()
            && self.open_world.is_none()
    }

    pub(crate) fn into_rmcp(self) -> rmcp::model::ToolAnnotations {
        // `rmcp::model::ToolAnnotations` is #[non_exhaustive]; `from_raw` is
        // its all-fields constructor.
        rmcp::model::ToolAnnotations::from_raw(
            self.title,
            self.read_only,
            self.destructive,
            self.idempotent,
            self.open_world,
        )
    }
}

/// One registered MCP tool: wire metadata plus its dispatch closure.
///
/// Produced by the `#[mcp_routes]` macro (one per `#[tool]` method); can also
/// be built by hand for dynamic tools.
#[derive(Clone)]
pub struct ToolRoute {
    /// Unique tool name (unique across ALL registered services — a duplicate
    /// is a boot panic).
    pub name: Cow<'static, str>,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Description shown to the agent (from the method's doc comment).
    pub description: Option<String>,
    /// JSON Schema (draft 2020-12) object for the tool arguments.
    pub input_schema: Arc<SchemaObject>,
    /// Optional JSON Schema for `structuredContent` of results.
    pub output_schema: Option<Arc<SchemaObject>>,
    /// Advisory behavior hints.
    pub annotations: ToolAnnotations,
    /// The dispatch closure.
    pub invoke: ToolInvoke,
}

impl ToolRoute {
    /// Convert the metadata into the rmcp wire `Tool` (dispatch closure
    /// excluded).
    pub(crate) fn to_rmcp_tool(&self) -> rmcp::model::Tool {
        let mut tool = rmcp::model::Tool::new(
            self.name.clone(),
            self.description.clone().unwrap_or_default(),
            Arc::clone(&self.input_schema),
        );
        tool.title = self.title.clone();
        if self.description.is_none() {
            tool.description = None;
        }
        tool.output_schema = self.output_schema.clone();
        if !self.annotations.is_empty() {
            tool.annotations = Some(self.annotations.clone().into_rmcp());
        }
        tool
    }
}

impl std::fmt::Debug for ToolRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRoute")
            .field("name", &self.name)
            .field("title", &self.title)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}
