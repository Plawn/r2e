//! The rmcp `ServerHandler` implementation dispatching to registered R2E
//! tools, resources and prompts.

use std::collections::HashMap;
use std::sync::Arc;

use r2e_core::http::Parts;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ErrorCode, ErrorData, GetPromptRequestParams,
    GetPromptResponse, Implementation, ListPromptsResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, Prompt, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;

use crate::auth::tools::{tool_visible, ToolRequirements};
use crate::registry::RegisteredMcpService;
use crate::route::{PromptCall, PromptRoute, ResourceCall, ResourceRoute, ToolCall, ToolRoute};

/// Server identity/behavior settings resolved by the plugin (builder
/// overrides > `mcp.*` config > defaults).
pub(crate) struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
}

/// One member family's dispatch table: routes by key, the precomputed list
/// payload, and the parallel requirements for the visibility filter.
struct Family<R, W> {
    routes: HashMap<String, R>,
    list: Vec<W>,
    reqs: Vec<ToolRequirements>,
    /// Per-caller list filtering (`mcp.auth.filter-tools`). Off — or no
    /// member has requirements — means the precomputed list goes out as-is.
    filter: bool,
}

impl<R, W: Clone> Family<R, W> {
    fn build(
        kind: &str,
        filter: bool,
        members: Vec<(&'static str, String, ToolRequirements, W, R)>,
    ) -> Self {
        let mut routes = HashMap::new();
        let mut owners: HashMap<String, &'static str> = HashMap::new();
        let mut list = Vec::new();
        let mut reqs = Vec::new();
        for (service, key, req, wire, route) in members {
            if let Some(previous) = owners.get(key.as_str()) {
                panic!(
                    "duplicate MCP {kind} `{key}`: registered by both `{previous}` and \
                     `{service}` — {kind}s are global across services"
                );
            }
            owners.insert(key.clone(), service);
            list.push(wire);
            reqs.push(req);
            routes.insert(key, route);
        }
        let filter = filter && reqs.iter().any(|r| !r.is_empty());
        Family {
            routes,
            list,
            reqs,
            filter,
        }
    }

    /// The list payload for this caller: precomputed when unfiltered,
    /// otherwise the members whose requirements the caller satisfies.
    fn visible_list(&self, context: &RequestContext<RoleServer>) -> Vec<W> {
        if !self.filter {
            return self.list.clone();
        }
        // The auth layer's principal travels in the HTTP request parts that
        // the transport copies into the request extensions.
        let extensions = context
            .extensions
            .get::<Parts>()
            .map(|parts| &parts.extensions);
        self.list
            .iter()
            .zip(self.reqs.iter())
            .filter(|(_, req)| tool_visible(extensions, req))
            .map(|(wire, _)| wire.clone())
            .collect()
    }
}

/// The immutable dispatch table built once when the router is assembled.
pub(crate) struct McpRuntime {
    info: ServerInfo,
    tools: Family<ToolRoute, Tool>,
    resources: Family<ResourceRoute, Resource>,
    prompts: Family<PromptRoute, Prompt>,
}

impl McpRuntime {
    /// Fold the drained services into one dispatch table.
    ///
    /// # Panics
    ///
    /// Panics when two services register the same tool name, resource URI or
    /// prompt name — the MCP equivalent of an HTTP route conflict, surfaced
    /// at boot with both service names.
    pub(crate) fn build(
        services: Vec<RegisteredMcpService>,
        identity: ServerIdentity,
        filter_tools: bool,
    ) -> Self {
        let mut tool_members = Vec::new();
        let mut resource_members = Vec::new();
        let mut prompt_members = Vec::new();
        for service in services {
            let name = service.name;
            for tool in service.routes.tools {
                tool_members.push((
                    name,
                    tool.name.to_string(),
                    tool.requirements,
                    tool.to_rmcp_tool(),
                    tool,
                ));
            }
            for resource in service.routes.resources {
                resource_members.push((
                    name,
                    resource.uri.to_string(),
                    resource.requirements,
                    resource.to_rmcp_resource(),
                    resource,
                ));
            }
            for prompt in service.routes.prompts {
                prompt_members.push((
                    name,
                    prompt.name.to_string(),
                    prompt.requirements,
                    prompt.to_rmcp_prompt(),
                    prompt,
                ));
            }
        }
        let tools = Family::build("tool name", filter_tools, tool_members);
        let resources = Family::build("resource URI", filter_tools, resource_members);
        let prompts = Family::build("prompt name", filter_tools, prompt_members);

        let mut info = ServerInfo::default();
        // Tools are always advertised (the endpoint's primary purpose);
        // resources/prompts only when at least one exists — the typestate
        // builder cannot enable conditionally, so set the pub fields.
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        if !resources.list.is_empty() {
            capabilities.resources = Some(Default::default());
        }
        if !prompts.list.is_empty() {
            capabilities.prompts = Some(Default::default());
        }
        info.capabilities = capabilities;
        info.server_info = Implementation::new(identity.name, identity.version);
        info.instructions = identity.instructions;

        McpRuntime {
            info,
            tools,
            resources,
            prompts,
        }
    }

    pub(crate) fn tool_names(&self) -> Vec<&str> {
        self.tools.list.iter().map(|t| t.name.as_ref()).collect()
    }

    pub(crate) fn resource_count(&self) -> usize {
        self.resources.list.len()
    }

    pub(crate) fn prompt_count(&self) -> usize {
        self.prompts.list.len()
    }
}

/// Extract the per-call HTTP parts from the request context, by value.
///
/// The streamable-HTTP transport inserts the originating HTTP request parts
/// into the request extensions on every path; taking them lets guards and
/// identity extraction see the real request head.
fn take_parts(context: &mut RequestContext<RoleServer>) -> Option<Arc<Parts>> {
    context.extensions.remove::<Parts>().map(Arc::new)
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
        Ok(ListToolsResult::with_all_items(
            self.rt.tools.visible_list(&context),
        ))
    }

    /// Returning the real `Tool` here opts into rmcp's input validation:
    /// arguments are checked against `inputSchema` before `call_tool` runs
    /// (SEP-2243).
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.rt.tools.routes.get(name).map(ToolRoute::to_rmcp_tool)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(route) = self.rt.tools.routes.get(request.name.as_ref()) else {
            // Unknown tool is a protocol error (unroutable request), per the
            // MCP convention rmcp documents on `call_tool`.
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {}", request.name),
                None,
            ));
        };

        let parts = take_parts(&mut context);
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

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(
            self.rt.resources.visible_list(&context),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let Some(route) = self.rt.resources.routes.get(request.uri.as_str()) else {
            // Unknown resource URI has its own JSON-RPC code per the spec.
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource: {}", request.uri),
                None,
            ));
        };

        let call = ResourceCall {
            uri: request.uri,
            parts: take_parts(&mut context),
            request_id: context.id.to_string(),
            cancel: context.ct.into(),
        };

        match (route.invoke)(call).await {
            Ok(contents) => Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                contents,
            ))),
            // Resources have no in-result error plane: every error is a
            // JSON-RPC error (`McpError::Tool` degrades to internal).
            Err(err) => Err(err.into_error_data()),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(
            self.rt.prompts.visible_list(&context),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let Some(route) = self.rt.prompts.routes.get(request.name.as_str()) else {
            // Unknown prompt name is invalid params per the MCP spec.
            return Err(ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                format!("unknown prompt: {}", request.name),
                None,
            ));
        };

        let arguments = match request.arguments {
            Some(map) => Value::Object(map),
            None => Value::Object(serde_json::Map::new()),
        };
        let call = PromptCall {
            arguments,
            parts: take_parts(&mut context),
            request_id: context.id.to_string(),
            cancel: context.ct.into(),
        };

        match (route.invoke)(call).await {
            Ok(result) => Ok(GetPromptResponse::Complete(result)),
            Err(err) => Err(err.into_error_data()),
        }
    }
}
