//! The rmcp `ServerHandler` implementation dispatching to registered R2E
//! tools.

use std::collections::HashMap;
use std::sync::Arc;

use r2e_core::http::Parts;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ErrorCode, ErrorData, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;

use crate::auth::tools::{tool_visible, ToolRequirements};
use crate::registry::RegisteredMcpService;
use crate::route::{ToolCall, ToolRoute};

/// Server identity/behavior settings resolved by the plugin (builder
/// overrides > `mcp.*` config > defaults).
pub(crate) struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
}

/// The immutable dispatch table built once when the router is assembled:
/// server info, tools by name, and the precomputed `tools/list` payload.
pub(crate) struct McpRuntime {
    info: ServerInfo,
    tools: HashMap<String, ToolRoute>,
    tool_list: Vec<Tool>,
    /// Requirements parallel to `tool_list`, for the visibility filter.
    tool_reqs: Vec<ToolRequirements>,
    /// Per-caller `tools/list` filtering (`mcp.auth.filter-tools`, default on
    /// when auth is enabled). When off — or when no tool has requirements —
    /// `list_tools` returns the precomputed list unfiltered.
    filter_tools: bool,
}

impl McpRuntime {
    /// Fold the drained services into one dispatch table.
    ///
    /// # Panics
    ///
    /// Panics when two services register the same tool name — the MCP
    /// equivalent of an HTTP route conflict, surfaced at boot with both
    /// service names.
    pub(crate) fn build(
        services: Vec<RegisteredMcpService>,
        identity: ServerIdentity,
        filter_tools: bool,
    ) -> Self {
        let mut tools: HashMap<String, ToolRoute> = HashMap::new();
        let mut owners: HashMap<String, &'static str> = HashMap::new();
        let mut tool_list = Vec::new();
        let mut tool_reqs = Vec::new();
        for service in services {
            for tool in service.tools {
                let name = tool.name.to_string();
                if let Some(previous) = owners.get(name.as_str()) {
                    panic!(
                        "duplicate MCP tool name `{name}`: registered by both `{previous}` \
                         and `{}` — tool names are global across services",
                        service.name
                    );
                }
                owners.insert(name.clone(), service.name);
                tool_list.push(tool.to_rmcp_tool());
                tool_reqs.push(tool.requirements);
                tools.insert(name, tool);
            }
        }

        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new(identity.name, identity.version);
        info.instructions = identity.instructions;

        // The filter only ever hides restricted tools; with none, skip the
        // per-request pass entirely.
        let filter_tools = filter_tools && tool_reqs.iter().any(|r| !r.is_empty());

        McpRuntime {
            info,
            tools,
            tool_list,
            tool_reqs,
            filter_tools,
        }
    }

    pub(crate) fn tool_names(&self) -> Vec<&str> {
        self.tool_list.iter().map(|t| t.name.as_ref()).collect()
    }
}

/// The `ServerHandler` handed to rmcp's streamable-HTTP service. Cheap to
/// clone (one `Arc`); the transport's session factory clones it per session.
#[derive(Clone)]
pub(crate) struct R2eMcpHandler {
    rt: Arc<McpRuntime>,
}

impl R2eMcpHandler {
    pub(crate) fn new(rt: Arc<McpRuntime>) -> Self {
        R2eMcpHandler { rt }
    }
}

impl ServerHandler for R2eMcpHandler {
    fn get_info(&self) -> ServerInfo {
        self.rt.info.clone()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if !self.rt.filter_tools {
            return Ok(ListToolsResult::with_all_items(self.rt.tool_list.clone()));
        }
        // Per-caller visibility: hide the tools whose scope/role
        // requirements the caller does not satisfy (`mcp.auth.filter-tools`).
        // The auth layer's principal travels in the HTTP request parts that
        // the transport copies into the request extensions.
        let extensions = context
            .extensions
            .get::<Parts>()
            .map(|parts| &parts.extensions);
        let visible = self
            .rt
            .tool_list
            .iter()
            .zip(self.rt.tool_reqs.iter())
            .filter(|(_, req)| tool_visible(extensions, req))
            .map(|(tool, _)| tool.clone())
            .collect();
        Ok(ListToolsResult::with_all_items(visible))
    }

    /// Returning the real `Tool` here opts into rmcp's input validation:
    /// arguments are checked against `inputSchema` before `call_tool` runs
    /// (SEP-2243).
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.rt.tools.get(name).map(ToolRoute::to_rmcp_tool)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(route) = self.rt.tools.get(request.name.as_ref()) else {
            // Unknown tool is a protocol error (unroutable request), per the
            // MCP convention rmcp documents on `call_tool`.
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {}", request.name),
                None,
            ));
        };

        // The streamable-HTTP transport inserts the originating HTTP request
        // parts into the request extensions on every path; take them by value
        // (they are per-call) so guards and identity extraction see the real
        // request head.
        let parts = context.extensions.remove::<Parts>().map(Arc::new);
        let arguments = match request.arguments {
            Some(map) => Value::Object(map),
            None => Value::Object(serde_json::Map::new()),
        };
        let call = ToolCall {
            arguments,
            parts,
            request_id: context.id.to_string(),
            cancel: context.ct.into(),
        };

        match (route.invoke)(call).await {
            Ok(result) => Ok(CallToolResponse::Complete(result)),
            Err(err) => err.into_call_result().map(CallToolResponse::Complete),
        }
    }
}
