//! The rmcp `ServerHandler` implementation dispatching to registered R2E
//! tools, resources and prompts.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use r2e_core::http::Parts;
use r2e_core::rt::sync::broadcast;
use r2e_core::rt::CancelToken;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CompleteRequestParams, CompleteResult, CompletionInfo,
    ErrorCode, ErrorData, GetPromptRequestParams, GetPromptResponse, InitializeRequestParams,
    InitializeResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Reference, ResourceUpdatedNotificationParam, ServerInfo,
    SubscribeRequestParams, SubscriptionFilter, Tool, UnsubscribeRequestParams,
};
use rmcp::service::{RequestContext, RoleServer, SubscriptionContext};
use serde_json::Value;

use crate::auth::tools::requirements_visible;
use crate::catalog::{principal_in, Catalog};
use crate::elicitation::ClientChannel;
use crate::error::McpError;
use crate::progress::Progress;
use crate::resource_updates::McpResourceUpdates;
use crate::route::{Completion, CompletionRef, Completions, PromptCall, ResourceCall, ToolCall};
use crate::session::{McpSession, McpSessions, SessionInitFn, SessionLink, SessionState};

/// Extract the per-call HTTP parts from the request context, by value.
///
/// The streamable-HTTP transport inserts the originating HTTP request parts
/// into the request extensions on every path; taking them lets guards and
/// identity extraction see the real request head.
fn take_parts(context: &mut RequestContext<RoleServer>) -> Option<Arc<Parts>> {
    context.extensions.remove::<Parts>().map(Arc::new)
}

/// The per-call context every member family shares — the one place a new
/// `ToolCall`/`ResourceCall`/`PromptCall` field is sourced from the rmcp
/// request context.
struct CallContext {
    parts: Option<Arc<Parts>>,
    request_id: String,
    cancel: CancelToken,
    session: McpSession,
    progress: Progress,
    channel: ClientChannel,
}

impl CallContext {
    fn new(
        mut context: RequestContext<RoleServer>,
        session: McpSession,
        elicitation_timeout: Duration,
    ) -> Self {
        let parts = take_parts(&mut context);
        let cancel: CancelToken = context.ct.into();
        CallContext {
            parts,
            request_id: context.id.to_string(),
            progress: Progress::new(context.meta.get_progress_token(), &context.peer),
            // Only a real session routes the client's answer back to this
            // peer; sessionless requests fail fast with `NoChannel`.
            channel: ClientChannel::new(
                &context.peer,
                session.is_persistent(),
                elicitation_timeout,
                &cancel,
            ),
            cancel,
            session,
        }
    }

    fn tool(self, arguments: Option<serde_json::Map<String, Value>>) -> ToolCall {
        ToolCall {
            arguments: Value::Object(arguments.unwrap_or_default()),
            parts: self.parts,
            request_id: self.request_id,
            cancel: self.cancel,
            session: Some(self.session),
            progress: self.progress,
            channel: self.channel,
        }
    }

    fn resource(self, uri: String, variables: BTreeMap<String, String>) -> ResourceCall {
        ResourceCall {
            uri,
            variables,
            parts: self.parts,
            request_id: self.request_id,
            cancel: self.cancel,
            session: Some(self.session),
            progress: self.progress,
            channel: self.channel,
        }
    }

    fn prompt(self, arguments: Option<serde_json::Map<String, Value>>) -> PromptCall {
        PromptCall {
            arguments: Value::Object(arguments.unwrap_or_default()),
            parts: self.parts,
            request_id: self.request_id,
            cancel: self.cancel,
            session: Some(self.session),
            progress: self.progress,
            channel: self.channel,
        }
    }
}

/// Everything the session handlers of one endpoint share.
pub(crate) struct Endpoint {
    pub(crate) catalog: Arc<Catalog>,
    pub(crate) updates: McpResourceUpdates,
    pub(crate) sessions: McpSessions,
    pub(crate) init: Option<SessionInitFn>,
    /// Transport-wide shutdown token (the plugin's `mcp_cancel`): ends the
    /// legacy forwarder task with the sessions it serves.
    pub(crate) shutdown: CancelToken,
    /// `mcp.elicitation-timeout-secs`.
    pub(crate) elicitation_timeout: Duration,
}

/// The `ServerHandler` handed to rmcp's streamable-HTTP service. rmcp's
/// factory builds one per MCP session (per request when sessionless), so
/// the handler owns the session's member list.
pub(crate) struct R2eMcpHandler {
    endpoint: Arc<Endpoint>,
    state: Arc<SessionState>,
    legacy_subscriptions: Arc<RwLock<HashSet<String>>>,
    /// Whether a forwarder task is currently alive for this session. Reset
    /// by the task itself on exit, so a later `resources/subscribe` starts a
    /// fresh one instead of silently succeeding with nothing delivering.
    legacy_forwarder_alive: Arc<AtomicBool>,
}

impl R2eMcpHandler {
    pub(crate) fn new(endpoint: Arc<Endpoint>) -> Self {
        R2eMcpHandler {
            state: Arc::new(SessionState::new(Arc::clone(&endpoint.catalog))),
            endpoint,
            legacy_subscriptions: Arc::new(RwLock::new(HashSet::new())),
            legacy_forwarder_alive: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Per-request session bookkeeping, before any member resolution:
    ///
    /// 1. capture the peer (the `list_changed` channel);
    /// 2. bind the session to the caller's subject on first use and refuse
    ///    a later request from another subject — a leaked session id must
    ///    not hand one principal another's list (or its private members).
    ///    Under `mcp.auth` the layer already refuses a foreign session id on
    ///    every HTTP method (`SessionBindings`); this is the backstop;
    /// 3. run the [`McpSessionInit`](crate::McpSessionInit) hook once.
    async fn prepare(&self, context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
        self.state.capture_peer(&context.peer);
        let parts = context.extensions.get::<Parts>();
        let subject = principal_in(parts).map(|p| p.user.sub.as_str());
        if !self.state.bind(subject) {
            tracing::warn!(
                bound = ?self.state.subject(),
                caller = ?subject,
                "MCP session id presented by a different principal; request refused"
            );
            return Err(McpError::Forbidden(
                "this MCP session belongs to another principal".into(),
            )
            .into_error_data());
        }
        if let Some(hook) = &self.endpoint.init {
            self.state
                .ensure_init(hook, parts)
                .await
                .map_err(McpError::into_error_data)?;
        }
        Ok(())
    }

    /// The session handle a member call carries.
    fn session(&self) -> McpSession {
        McpSession::new(Arc::clone(&self.state))
    }
}

/// Legacy (pre-2026-07-28) `resources/subscribe` delivery for one session:
/// relays published updates whose URI is in the session's set to the peer.
/// Ends with the transport (shutdown token / peer gone), when the session's
/// handler is dropped, or on a delivery failure — never leaving a dead task
/// counted as "alive".
async fn forward_legacy_updates(
    mut updates: broadcast::Receiver<String>,
    subscriptions: std::sync::Weak<RwLock<HashSet<String>>>,
    peer: rmcp::service::Peer<RoleServer>,
    shutdown: CancelToken,
) {
    loop {
        let update = r2e_core::rt::select! {
            _ = shutdown.cancelled() => return,
            update = updates.recv() => update,
            _ = r2e_core::rt::sleep(std::time::Duration::from_secs(30)) => {
                if subscriptions.strong_count() == 0 || peer.is_transport_closed() {
                    return;
                }
                continue;
            }
        };
        match update {
            Ok(uri) => {
                let Some(subscriptions) = subscriptions.upgrade() else {
                    return;
                };
                let subscribed = subscriptions
                    .read()
                    .expect("MCP subscription lock poisoned")
                    .contains(&uri);
                #[allow(deprecated)]
                if subscribed
                    && peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Relay an endpoint-wide list change ([`McpSessions::notify_list_changed`])
/// to a sessionless `subscriptions/listen` stream, for each family the
/// client subscribed to. `false` when the stream is gone.
async fn forward_list_changed(context: &SubscriptionContext) -> bool {
    let accepted = context.accepted();
    let sink = context.sink();
    if accepted.tools_list_changed == Some(true) && sink.notify_tool_list_changed().await.is_err() {
        return false;
    }
    if accepted.resources_list_changed == Some(true)
        && sink.notify_resource_list_changed().await.is_err()
    {
        return false;
    }
    if accepted.prompts_list_changed == Some(true)
        && sink.notify_prompt_list_changed().await.is_err()
    {
        return false;
    }
    true
}

impl ServerHandler for R2eMcpHandler {
    fn get_info(&self) -> ServerInfo {
        self.endpoint.catalog.info.clone()
    }

    /// rmcp's default, plus session opening: under stateful serving rmcp
    /// only hands the handshake to a handler it built for a new (or
    /// restored) session, so this is where the session becomes persistent
    /// and reachable through [`McpSessions`]. [`prepare`](Self::prepare)
    /// then binds it to its opener's subject, before the client even knows
    /// its id.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        if self.state.open() {
            self.endpoint.sessions.register(&self.state);
            // Under `mcp.auth`, let the layer key this session by the id
            // rmcp is about to assign (see `SessionBindings`).
            if let Some(link) = context
                .extensions
                .get::<Parts>()
                .and_then(|parts| parts.extensions.get::<SessionLink>())
            {
                link.set(&self.state);
            }
        }
        self.prepare(&context).await?;
        context.peer.set_peer_info(request.clone());
        let mut info = self.get_info();
        if self
            .supported_protocol_versions()
            .contains(&request.protocol_version)
        {
            info.protocol_version = request.protocol_version;
        } else {
            tracing::warn!(
                client_requested = %request.protocol_version,
                server_fallback = %info.protocol_version,
                "client requested unsupported protocol version; falling back to server default"
            );
        }
        Ok(info)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.prepare(&context).await?;
        Ok(ListToolsResult::with_all_items(
            self.state.view().tools.visible_list(&context),
        ))
    }

    /// Returning the real `Tool` lets rmcp validate `Mcp-Param-*` headers
    /// against `inputSchema` (SEP-2243). rmcp asks a fresh handler and
    /// caches the answer per name, so this is catalog-wide (hidden groups
    /// included, session-private tools never): visibility is enforced by
    /// `call_tool`.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.endpoint.catalog.tool_wire(name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.prepare(&context).await?;
        let view = self.state.view();
        // A member hidden from this session answers exactly like an unknown
        // one — no enumeration oracle.
        let Some(route) = view.tools.route(request.name.as_ref()) else {
            // Unknown tool is a protocol error (unroutable request), per the
            // MCP convention rmcp documents on `call_tool`.
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {}", request.name),
                None,
            ));
        };

        let call = CallContext::new(context, self.session(), self.endpoint.elicitation_timeout)
            .tool(request.arguments);

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
        self.prepare(&context).await?;
        Ok(ListResourcesResult::with_all_items(
            self.state.view().resources.visible_list(&context),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.prepare(&context).await?;
        Ok(ListResourceTemplatesResult::with_all_items(
            self.state.view().resource_templates.visible_list(&context),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.prepare(&context).await?;
        let view = self.state.view();
        let Some((route, variables)) = view.resource_route(request.uri.as_str()) else {
            // Unknown resource URI has its own JSON-RPC code per the spec.
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource: {}", request.uri),
                None,
            ));
        };

        let call = CallContext::new(context, self.session(), self.endpoint.elicitation_timeout)
            .resource(request.uri, variables);

        match (route.invoke)(call).await {
            Ok(contents) => Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                contents,
            ))),
            // Resources have no in-result error plane: every error is a
            // JSON-RPC error (`McpError::Tool` degrades to internal).
            Err(err) => Err(err.into_error_data()),
        }
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let mut accepted = requested.supported_by(&self.endpoint.catalog.info.capabilities);
        if let Some(uris) = accepted.resource_subscriptions.as_mut() {
            let view = self.state.view();
            uris.retain(|uri| view.has_resource(uri));
            if uris.is_empty() {
                accepted.resource_subscriptions = None;
            }
        }
        Some(accepted)
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let mut updates = self.endpoint.updates.subscribe();
        let mut list_changes = self.endpoint.sessions.subscribe_changes();
        loop {
            r2e_core::rt::select! {
                _ = context.cancelled() => return Ok(()),
                change = list_changes.recv() => match change {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !forward_list_changed(&context).await {
                            return Ok(());
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                },
                update = updates.recv() => match update {
                    Ok(uri) => {
                        if context.accepted().resource_subscriptions.as_ref()
                            .is_some_and(|uris| uris.contains(&uri))
                            && context.sink().notify_resource_updated(uri).await.is_err()
                        {
                            return Ok(());
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.prepare(&context).await?;
        if !self.state.view().has_resource(&request.uri) {
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource: {}", request.uri),
                None,
            ));
        }
        self.legacy_subscriptions
            .write()
            .expect("MCP subscription lock poisoned")
            .insert(request.uri);

        if !self.legacy_forwarder_alive.swap(true, Ordering::AcqRel) {
            let alive = Arc::clone(&self.legacy_forwarder_alive);
            let subscriptions = Arc::downgrade(&self.legacy_subscriptions);
            let updates = self.endpoint.updates.subscribe();
            let shutdown = self.endpoint.shutdown.clone();
            let peer = context.peer;
            r2e_core::rt::spawn_ctl(async move {
                forward_legacy_updates(updates, subscriptions, peer, shutdown).await;
                alive.store(false, Ordering::Release);
            });
        }
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.prepare(&context).await?;
        self.legacy_subscriptions
            .write()
            .expect("MCP subscription lock poisoned")
            .remove(&request.uri);
        Ok(())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.prepare(&context).await?;
        Ok(ListPromptsResult::with_all_items(
            self.state.view().prompts.visible_list(&context),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.prepare(&context).await?;
        let view = self.state.view();
        let Some(route) = view.prompts.route(request.name.as_str()) else {
            // Unknown prompt name is invalid params per the MCP spec.
            return Err(ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                format!("unknown prompt: {}", request.name),
                None,
            ));
        };

        let call = CallContext::new(context, self.session(), self.endpoint.elicitation_timeout)
            .prompt(request.arguments);

        match (route.invoke)(call).await {
            Ok(result) => Ok(GetPromptResponse::Complete(result)),
            Err(err) => Err(err.into_error_data()),
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        self.prepare(&context).await?;
        let view = self.state.view();
        let target = match &request.r#ref {
            Reference::Prompt(prompt) => view.prompts.route(&prompt.name).map(|route| {
                (
                    CompletionRef::Prompt(prompt.name.clone()),
                    &route.requirements,
                    &route.completions,
                )
            }),
            Reference::Resource(resource) => view.completion_resource(&resource.uri).map(|route| {
                (
                    CompletionRef::Resource(resource.uri.clone()),
                    &route.requirements,
                    &route.completions,
                )
            }),
            _ => None,
        };
        // A member the caller may not use gets exactly the unknown-reference
        // error: completion values must not reveal a hidden member, whether
        // hidden by its group or by its requirements (checked here even
        // without `mcp.auth.filter-members`).
        let principal = principal_in(context.extensions.get::<Parts>());
        let Some((reference, _, providers)) =
            target.filter(|(_, requirements, _)| requirements_visible(principal, requirements))
        else {
            return Err(ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                "unknown completion reference",
                None,
            ));
        };
        let Some(invoke) = providers
            .iter()
            .find(|p| p.argument == request.argument.name)
            .map(|p| Arc::clone(&p.invoke))
        else {
            return Ok(CompleteResult::new(CompletionInfo::default()));
        };
        drop(view);

        let parts = take_parts(&mut context);
        let completion = Completion {
            reference,
            argument: request.argument.name,
            value: request.argument.value,
            context: request
                .context
                .and_then(|c| c.arguments)
                .map(|arguments| arguments.into_iter().collect())
                .unwrap_or_default(),
            parts,
            request_id: context.id.to_string(),
            cancel: context.ct.into(),
            session: Some(self.session()),
        };
        let completions = invoke(completion)
            .await
            .map_err(McpError::into_error_data)?;
        Ok(CompleteResult::new(completion_info(completions)))
    }
}

/// The wire form of a provider's suggestions, capped at the spec's 100
/// values; truncating implies `hasMore` and a `total` (the provider's own,
/// else the untruncated count).
fn completion_info(completions: Completions) -> CompletionInfo {
    let Completions {
        mut values,
        total,
        has_more,
    } = completions;
    let count = values.len();
    let truncated = count > CompletionInfo::MAX_VALUES;
    values.truncate(CompletionInfo::MAX_VALUES);
    let mut info = CompletionInfo::default();
    info.values = values;
    info.total = total.or_else(|| truncated.then(|| u32::try_from(count).unwrap_or(u32::MAX)));
    info.has_more = (has_more || truncated).then_some(true);
    info
}
