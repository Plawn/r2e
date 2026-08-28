//! Tool routes: the unit `#[mcp_routes]` compiles each `#[tool]` method into.
//!
//! A [`ToolRoute`] carries the tool's wire metadata (name, description,
//! schemas, annotations) plus a type-erased [`ToolInvoke`] closure that owns
//! the service wrapper and runs the actual method. The handler
//! ([`crate::handler`]) precomputes the rmcp `Tool` list from the metadata and
//! dispatches `tools/call` to the closure.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use r2e_core::http::Parts;
use r2e_core::rt::CancelToken;
use rmcp::model::{CallToolResult, GetPromptResult, ResourceContents};
use serde_json::Value;

use crate::auth::ToolRequirements;
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
    /// Authorization requirements (`#[tool(scopes/any_scopes)]` +
    /// `#[roles]`/`#[all_roles]`), checked in the invoke prologue and used
    /// by the `tools/list` visibility filter. [`ToolRequirements::NONE`] for
    /// unrestricted tools.
    pub requirements: ToolRequirements,
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

// ── Resources ──────────────────────────────────────────────────────────────

/// Everything a resource read can observe about its call.
///
/// The resource counterpart of [`ToolCall`]: handed to the generated dispatch
/// closure; resource methods can also take it as a parameter to read the
/// requested URI, HTTP request parts, or the cancellation token.
#[derive(Clone)]
pub struct ResourceCall {
    /// The URI of the `resources/read` request.
    pub uri: String,
    /// Variables captured from an RFC 6570 URI template. Empty for a fixed
    /// resource URI.
    pub variables: BTreeMap<String, String>,
    /// The HTTP request parts of the transport request carrying this call —
    /// same semantics as [`ToolCall::parts`].
    pub parts: Option<Arc<Parts>>,
    /// The JSON-RPC request id, stringified (for logging/correlation).
    pub request_id: String,
    /// Cancelled when the client aborts the request or the server shuts
    /// down.
    pub cancel: CancelToken,
}

impl ResourceCall {
    /// Read a request-scoped value of type `T` from the HTTP request
    /// extensions — same semantics as [`ToolCall::extension`].
    pub fn extension<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.parts.as_ref()?.extensions.get::<T>().cloned()
    }
}

/// Boxed future returned by a resource read.
pub type ResourceFuture =
    Pin<Box<dyn Future<Output = Result<Vec<ResourceContents>, McpError>> + Send>>;

/// Type-erased resource read closure. Owns (an `Arc` of) the service
/// wrapper; called once per `resources/read`.
pub type ResourceInvoke = Arc<dyn Fn(ResourceCall) -> ResourceFuture + Send + Sync>;

/// One registered MCP resource: wire metadata plus its read closure.
///
/// Produced by the `#[mcp_routes]` macro (one per `#[resource]` method); can
/// also be built by hand for dynamic resources.
#[derive(Clone)]
pub struct ResourceRoute {
    /// A fixed resource URI or RFC 6570 URI template (unique across all
    /// registered services — a duplicate is a boot panic).
    pub uri: Cow<'static, str>,
    /// The programmatic resource name (defaults to the method name).
    pub name: Cow<'static, str>,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Description shown to the agent (from the method's doc comment).
    pub description: Option<String>,
    /// The MIME type of the contents, advertised in `resources/list` and
    /// applied to text-shaped return values.
    pub mime_type: Option<String>,
    /// Authorization requirements (`#[resource(scopes/any_scopes)]` +
    /// `#[roles]`/`#[all_roles]`) — checked in the read prologue and used by
    /// the `resources/list` visibility filter.
    pub requirements: ToolRequirements,
    /// The read closure.
    pub invoke: ResourceInvoke,
}

impl ResourceRoute {
    /// Convert the metadata into the rmcp wire `Resource` (read closure
    /// excluded).
    pub(crate) fn to_rmcp_resource(&self) -> rmcp::model::Resource {
        let mut resource = rmcp::model::Resource::new(self.uri.clone(), self.name.clone());
        resource.title = self.title.clone();
        resource.description = self.description.clone();
        resource.mime_type = self.mime_type.clone();
        resource
    }

    /// Whether this route is advertised through `resources/templates/list`.
    pub fn is_template(&self) -> bool {
        self.uri.contains('{')
    }

    pub(crate) fn to_rmcp_resource_template(&self) -> rmcp::model::ResourceTemplate {
        let mut resource =
            rmcp::model::ResourceTemplate::new(self.uri.to_string(), self.name.to_string());
        resource.title = self.title.clone();
        resource.description = self.description.clone();
        resource.mime_type = self.mime_type.clone();
        resource
    }
}

impl std::fmt::Debug for ResourceRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRoute")
            .field("uri", &self.uri)
            .field("name", &self.name)
            .field("mime_type", &self.mime_type)
            .finish_non_exhaustive()
    }
}

// ── Prompts ────────────────────────────────────────────────────────────────

/// Everything a prompt expansion can observe about its call.
///
/// The prompt counterpart of [`ToolCall`] (same shape: prompt arguments are
/// a JSON object, though the MCP spec constrains the values to strings).
#[derive(Clone)]
pub struct PromptCall {
    /// The raw `arguments` object of the `prompts/get` request (an empty
    /// JSON object when the client sent none). Per the MCP spec, values are
    /// strings.
    pub arguments: Value,
    /// The HTTP request parts of the transport request carrying this call —
    /// same semantics as [`ToolCall::parts`].
    pub parts: Option<Arc<Parts>>,
    /// The JSON-RPC request id, stringified (for logging/correlation).
    pub request_id: String,
    /// Cancelled when the client aborts the request or the server shuts
    /// down.
    pub cancel: CancelToken,
}

impl PromptCall {
    /// Read a request-scoped value of type `T` from the HTTP request
    /// extensions — same semantics as [`ToolCall::extension`].
    pub fn extension<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.parts.as_ref()?.extensions.get::<T>().cloned()
    }
}

/// Boxed future returned by a prompt expansion.
pub type PromptFuture = Pin<Box<dyn Future<Output = Result<GetPromptResult, McpError>> + Send>>;

/// Type-erased prompt expansion closure. Owns (an `Arc` of) the service
/// wrapper; called once per `prompts/get`.
pub type PromptInvoke = Arc<dyn Fn(PromptCall) -> PromptFuture + Send + Sync>;

/// One declared argument of a prompt (MCP `PromptArgument`).
///
/// Derived by the macro from the `Params<T>` schema: one entry per property,
/// `required` from the schema's `required` array, descriptions from
/// `#[doc]`/`schemars` descriptions.
#[derive(Debug, Clone, Default)]
pub struct PromptArgumentDef {
    /// The argument name.
    pub name: String,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Whether the argument must be provided.
    pub required: bool,
}

/// One registered MCP prompt: wire metadata plus its expansion closure.
///
/// Produced by the `#[mcp_routes]` macro (one per `#[prompt]` method); can
/// also be built by hand for dynamic prompts.
#[derive(Clone)]
pub struct PromptRoute {
    /// Unique prompt name (unique across ALL registered services — a
    /// duplicate is a boot panic).
    pub name: Cow<'static, str>,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Description shown to the agent (from the method's doc comment).
    pub description: Option<String>,
    /// The declared arguments, advertised in `prompts/list`.
    pub arguments: Vec<PromptArgumentDef>,
    /// Authorization requirements (`#[prompt(scopes/any_scopes)]` +
    /// `#[roles]`/`#[all_roles]`) — checked in the expansion prologue and
    /// used by the `prompts/list` visibility filter.
    pub requirements: ToolRequirements,
    /// The expansion closure.
    pub invoke: PromptInvoke,
}

impl PromptRoute {
    /// Convert the metadata into the rmcp wire `Prompt` (expansion closure
    /// excluded).
    pub(crate) fn to_rmcp_prompt(&self) -> rmcp::model::Prompt {
        let arguments = (!self.arguments.is_empty()).then(|| {
            self.arguments
                .iter()
                .map(|arg| {
                    let mut out = rmcp::model::PromptArgument::new(arg.name.clone());
                    out.title = arg.title.clone();
                    out.description = arg.description.clone();
                    out.required = Some(arg.required);
                    out
                })
                .collect()
        });
        let mut prompt =
            rmcp::model::Prompt::new(self.name.clone(), self.description.clone(), arguments);
        prompt.title = self.title.clone();
        prompt
    }
}

impl std::fmt::Debug for PromptRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptRoute")
            .field("name", &self.name)
            .field("arguments", &self.arguments)
            .finish_non_exhaustive()
    }
}

// ── Route bundle ───────────────────────────────────────────────────────────

/// Everything one MCP service contributes to the endpoint: its tool,
/// resource and prompt routes, built together from the bean graph (one
/// service core, one set of prebuilt decorators) by
/// [`McpService::routes`](crate::McpService::routes).
#[derive(Clone, Default)]
pub struct McpRoutes {
    /// `#[tool]` routes.
    pub tools: Vec<ToolRoute>,
    /// `#[resource]` routes.
    pub resources: Vec<ResourceRoute>,
    /// `#[prompt]` routes.
    pub prompts: Vec<PromptRoute>,
}

impl McpRoutes {
    /// A bundle containing only tool routes (the common hand-built case).
    pub fn from_tools(tools: Vec<ToolRoute>) -> Self {
        McpRoutes {
            tools,
            ..Default::default()
        }
    }
}
