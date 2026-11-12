//! The rmcp `ServerHandler` implementation dispatching to registered R2E
//! tools, resources and prompts.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

use r2e_core::http::Parts;
use r2e_core::rt::sync::broadcast;
use r2e_core::rt::CancelToken;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CompleteRequestParams, CompleteResult, CompletionInfo,
    ErrorCode, ErrorData, GetPromptRequestParams, GetPromptResponse, InitializeRequestParams,
    InitializeResult, InputRequiredResult, InputResponses, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Reference, ResourceUpdatedNotificationParam,
    ServerConfig, SubscribeRequestParams, SubscriptionFilter, Tool, UnsubscribeRequestParams,
};
use rmcp::service::{RequestContext, RoleServer, SubscriptionContext};
use rmcp::transport::streamable_http_server::SessionRestoreMarker;
use serde_json::Value;

use crate::auth::tools::visible_to;
use crate::catalog::{caller_in, Catalog, Changed};
use crate::elicitation::{advertised_modes, Back, ClientChannel};
use crate::error::McpError;
use crate::global::McpRegistry;
use crate::mrtr::{self, Mrtr, MrtrCall};
use crate::pagination::{paginate, ListKey};
use crate::progress::Progress;
use crate::resource_updates::McpResourceUpdates;
use crate::route::{
    Completion, CompletionRef, Completions, PromptCall, ResourceCall, ResourceRoute, ToolCall,
};
use crate::session::{
    McpSession, McpSessions, SessionBindings, SessionId, SessionInitFn, SessionLink,
    SessionPersistence, SessionState, SESSION_ID_HEADER,
};

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

/// What a call brings for multi-round-trip input (MRTR): who it is for —
/// the binding of its `requestState` — and the answers of a retry.
struct Input<'a> {
    method: &'static str,
    /// Tool or prompt name, resource URI.
    target: &'a str,
    arguments: Option<&'a serde_json::Map<String, Value>>,
    responses: Option<InputResponses>,
    state: Option<String>,
}

impl CallContext {
    /// The call context, and the MRTR round to close with
    /// [`Mrtr::finish`] once the member returned (`None` when the call is
    /// not served in rounds).
    fn new(
        mut context: RequestContext<RoleServer>,
        session: McpSession,
        endpoint: &Endpoint,
        input: Input<'_>,
    ) -> Result<(Self, Option<MrtrCall>), ErrorData> {
        // rmcp refuses a server→client request on 2026-07-28 and an
        // `input_required` result below it: the revision alone decides.
        let rounds = context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        let (back, mrtr) = match (rounds, &endpoint.mrtr) {
            (true, Some(mrtr)) => {
                let call = mrtr.begin(
                    mrtr::binding(
                        input.method,
                        input.target,
                        session.subject(),
                        input.arguments,
                    ),
                    input.responses,
                    input.state.as_deref(),
                )?;
                let back = Back::Rounds {
                    round: Arc::clone(&call.round),
                    modes: advertised_modes(context.client_capabilities().as_ref()),
                };
                (back, Some(call))
            }
            // No `mcp.request-state-key`: MRTR is off, `NoChannel`.
            (true, None) => (Back::None, None),
            // Only a real session routes the client's answer back to this
            // peer; sessionless requests fail fast with `NoChannel`.
            (false, _) if session.is_persistent() => (Back::Live, None),
            (false, _) => (Back::None, None),
        };
        let parts = take_parts(&mut context);
        let cancel: CancelToken = context.ct.into();
        let call = CallContext {
            parts,
            request_id: context.id.to_string(),
            progress: Progress::new(context.meta.get_progress_token(), &context.peer),
            channel: ClientChannel::new(
                &context.peer,
                back,
                endpoint.elicitation_timeout,
                &cancel,
            ),
            cancel,
            session,
        };
        Ok((call, mrtr))
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
    /// The endpoint-wide dynamic members, served by every session behind
    /// its own.
    pub(crate) registry: McpRegistry,
    pub(crate) updates: McpResourceUpdates,
    pub(crate) sessions: McpSessions,
    /// Session id → session, the auth/identity layers' ownership check. A
    /// session rmcp restores from the store is bound here by the handler
    /// (no `initialize` response carries its id through the layers).
    pub(crate) bindings: SessionBindings,
    pub(crate) init: Option<SessionInitFn>,
    /// Transport-wide shutdown token (the plugin's `mcp_cancel`): ends the
    /// legacy forwarder task with the sessions it serves.
    pub(crate) shutdown: CancelToken,
    /// `mcp.elicitation-timeout-secs`.
    pub(crate) elicitation_timeout: Duration,
    /// Multi-round-trip input for 2026-07-28 clients; `None` while
    /// `mcp.request-state-key` is not set.
    pub(crate) mrtr: Option<Mrtr>,
    /// `mcp.page-size` (`None` = unpaginated lists).
    pub(crate) page_size: Option<usize>,
    /// The protocol revisions advertised and accepted (`server/discover`,
    /// `initialize`, per-request `_meta` gate): rmcp's known list, capped by
    /// `mcp.max-protocol-version`.
    pub(crate) supported_versions: &'static [ProtocolVersion],
}

impl Endpoint {
    /// The result that replaces the member's own when it asked the client a
    /// question this round left unanswered.
    fn input_required(
        &self,
        rounds: Option<MrtrCall>,
    ) -> Result<Option<InputRequiredResult>, ErrorData> {
        match (&self.mrtr, rounds) {
            (Some(mrtr), Some(call)) => mrtr.finish(call),
            _ => Ok(None),
        }
    }
}

/// The `ServerHandler` handed to rmcp's streamable-HTTP service. rmcp's
/// factory builds one per MCP session (per request when sessionless), so
/// the handler owns the session's member list.
pub(crate) struct R2eMcpHandler {
    endpoint: Arc<Endpoint>,
    state: Arc<SessionState>,
    /// The session store wiring, on the HTTP transport only (a loopback
    /// session lives and dies with its process).
    persistence: Option<Arc<SessionPersistence>>,
    legacy_subscriptions: Arc<RwLock<HashMap<String, Weak<ResourceRoute>>>>,
    /// Whether a forwarder task is currently alive for this session. Reset
    /// by the task itself on exit, so a later `resources/subscribe` starts a
    /// fresh one instead of silently succeeding with nothing delivering.
    legacy_forwarder_alive: Arc<AtomicBool>,
}

/// rmcp drops the handler when the session ends (`DELETE`, idle eviction,
/// a failed restore): the state is closed then, so a handle that still
/// holds it ([`McpSession`](crate::McpSession)) does not keep the id live in
/// [`McpSessions`](crate::McpSessions) and `SessionBindings` — a same-id
/// session restored on this instance is the only one found.
impl Drop for R2eMcpHandler {
    fn drop(&mut self) {
        self.state.close();
    }
}

impl R2eMcpHandler {
    pub(crate) fn new(endpoint: Arc<Endpoint>) -> Self {
        R2eMcpHandler {
            state: Arc::new(SessionState::new(
                Arc::clone(&endpoint.catalog),
                endpoint.registry.clone(),
            )),
            endpoint,
            persistence: None,
            legacy_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            legacy_forwarder_alive: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A handler whose session is kept restorable in an
    /// [`McpSessionStore`](crate::McpSessionStore): the HTTP factory's
    /// constructor when a store is set.
    pub(crate) fn with_persistence(
        endpoint: Arc<Endpoint>,
        persistence: Arc<SessionPersistence>,
    ) -> Self {
        let mut handler = Self::new(endpoint);
        handler.persistence = Some(persistence);
        handler
    }

    /// The session this handler serves (a loopback connect names and claims
    /// it before the first message).
    #[cfg(feature = "loopback")]
    pub(crate) fn state(&self) -> &Arc<SessionState> {
        &self.state
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
        if let Some(id) = parts
            .and_then(|parts| parts.headers.get(SESSION_ID_HEADER))
            .and_then(|v| v.to_str().ok())
        {
            self.state.set_id(id);
        }
        let subject = caller_in(parts).map(|c| c.sub);
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
        // An active session stays restorable: re-arm its record once per
        // half-life. Only after the handshake was persisted (`None` before
        // — a restore replay reaches here before the peer info exists).
        if let Some(persistence) = &self.persistence
            && self.state.is_persistent()
            && let Some(id) = self.state.id()
            && self
                .state
                .persisted_since()
                .is_some_and(|since| since >= persistence.ttl() / 2)
        {
            persistence.touch(id, &self.state);
        }
        Ok(())
    }

    /// Resource interests do not grant access: resolve against the initialized
    /// view and enforce scope/role visibility even when list filtering is off.
    fn resource_visible(&self, uri: &str, context: &RequestContext<RoleServer>) -> bool {
        let caller = caller_in(context.extensions.get::<Parts>());
        self.state
            .resource(uri)
            .is_some_and(|(route, _)| visible_to(caller.as_ref(), &route.requirements))
    }

    /// The session handle a member call carries.
    fn session(&self) -> McpSession {
        McpSession::new(Arc::clone(&self.state))
    }

    /// The page of a `*/list` result the request's cursor asks for
    /// (`mcp.page-size`; the whole list when unset).
    fn page<W: ListKey>(
        &self,
        list: Vec<W>,
        request: Option<PaginatedRequestParams>,
        context: &RequestContext<RoleServer>,
    ) -> Result<(Vec<W>, Option<String>), ErrorData> {
        let subject = caller_in(context.extensions.get::<Parts>()).map(|c| c.sub);
        let cursor = request.and_then(|r| r.cursor);
        paginate(list, cursor.as_deref(), self.endpoint.page_size, subject)
    }
}

/// Legacy (pre-2026-07-28) `resources/subscribe` delivery for one session:
/// relays published updates whose URI is in the session's set to the peer.
/// Access was checked at `subscribe` (the session is bound to its
/// principal); each update is re-checked against the current view so a
/// resource removed or replaced after subscribing invalidates the subscription.
/// Weak route identity ties authorization to the exact route checked at subscribe.
/// Ends with the transport (shutdown token / peer gone), when the session's
/// handler is dropped, or on a delivery failure — never leaving a dead task
/// counted as "alive".
async fn forward_legacy_updates(
    mut updates: broadcast::Receiver<String>,
    subscriptions: Weak<RwLock<HashMap<String, Weak<ResourceRoute>>>>,
    state: std::sync::Weak<SessionState>,
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
                let subscribed = {
                    let mut subscriptions = subscriptions
                        .write()
                        .expect("MCP subscription lock poisoned");
                    let matches = subscriptions.get(&uri).is_some_and(|subscribed_route| {
                        state.upgrade().is_some_and(|state| {
                            state.resource(&uri).is_some_and(|(route, _)| {
                                Weak::ptr_eq(subscribed_route, &Arc::downgrade(&route))
                            })
                        })
                    });
                    if !matches {
                        subscriptions.remove(&uri);
                    }
                    matches
                };
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

/// Relay an endpoint-wide list change ([`McpSessions::notify_list_changed`],
/// a [`McpRegistry`] edit) to a sessionless `subscriptions/listen` stream,
/// for each changed family the client subscribed to. `false` when the
/// stream is gone.
async fn forward_list_changed(context: &SubscriptionContext, changed: Changed) -> bool {
    let accepted = context.accepted();
    let sink = context.sink();
    if changed.tools
        && accepted.tools_list_changed == Some(true)
        && sink.notify_tool_list_changed().await.is_err()
    {
        return false;
    }
    if changed.resources
        && accepted.resources_list_changed == Some(true)
        && sink.notify_resource_list_changed().await.is_err()
    {
        return false;
    }
    if changed.prompts
        && accepted.prompts_list_changed == Some(true)
        && sink.notify_prompt_list_changed().await.is_err()
    {
        return false;
    }
    true
}

impl ServerHandler for R2eMcpHandler {
    fn get_info(&self) -> ServerConfig {
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
        // rmcp replays the handshake of a session it restored from the
        // store with the restoring request's parts and this marker.
        let restored = context
            .extensions
            .get::<SessionRestoreMarker>()
            .map(|marker| SessionId::from(&*marker.id));
        if self.state.open() {
            self.endpoint.sessions.register(&self.state);
            match &restored {
                // The layers never saw an `initialize` response for this
                // id: bind it here, before the request that triggered the
                // restore (or a concurrent one) is served.
                Some(id) => {
                    self.state.set_id(id);
                    self.state.mark_restored();
                    self.endpoint
                        .bindings
                        .insert(id, Arc::downgrade(&self.state));
                }
                // Under `mcp.auth`, let the layer key this session by the
                // id rmcp is about to assign (see `SessionBindings`).
                None => {
                    if let Some(link) = context
                        .extensions
                        .get::<Parts>()
                        .and_then(|parts| parts.extensions.get::<SessionLink>())
                    {
                        link.set(&self.state);
                    }
                }
            }
        }
        self.prepare(&context).await?;
        // rmcp's negotiation: the requested revision when it is supported and
        // still has a handshake, else the newest supported one that does.
        let info = self.negotiate_initialize(&request)?;
        context.peer.set_peer_info(request);
        // Only a completed handshake is worth a record: rmcp persists the
        // session right after this returns whatever it answered, and the
        // adapter checks this flag. A fresh session is written there; a
        // restored one re-arms its record now (the peer info exists).
        self.state.mark_initialized();
        if let (Some(persistence), Some(id)) = (&self.persistence, &restored) {
            persistence.touch(id, &self.state);
        }
        Ok(info)
    }

    /// `mcp.max-protocol-version`: the one override behind `server/discover`,
    /// the `initialize` negotiation and the per-request `-32022` gate.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(self.endpoint.supported_versions)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.prepare(&context).await?;
        let list = self.state.tool_list(&context);
        let (page, next_cursor) = self.page(list, request, &context)?;
        let mut result = ListToolsResult::with_all_items(page);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    /// Returning the real `Tool` lets rmcp validate `Mcp-Param-*` headers
    /// against `inputSchema` (SEP-2243). rmcp asks a fresh handler and
    /// caches the answer per name, so this is catalog-wide (hidden groups
    /// included, session-private tools never) plus the global registry's
    /// current tools: visibility is enforced by `call_tool`. The cache
    /// means a registry tool's schema is the one rmcp first saw under that
    /// name — `Mcp-Param-*` headers are a convenience, the body is
    /// authoritative.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.endpoint
            .catalog
            .tool_wire(name)
            .or_else(|| self.endpoint.registry.view().tools.wire(name))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.prepare(&context).await?;
        // A member hidden from this session answers exactly like an unknown
        // one — no enumeration oracle.
        let Some(route) = self.state.tool(request.name.as_ref()) else {
            // Unknown tool is a protocol error (unroutable request), per the
            // MCP convention rmcp documents on `call_tool`.
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {}", request.name),
                None,
            ));
        };

        let (call, rounds) = CallContext::new(
            context,
            self.session(),
            &self.endpoint,
            Input {
                method: "tools/call",
                target: request.name.as_ref(),
                arguments: request.arguments.as_ref(),
                responses: request.input_responses,
                state: request.request_state,
            },
        )?;

        let outcome = (route.invoke)(call.tool(request.arguments)).await;
        if let Some(required) = self.endpoint.input_required(rounds)? {
            return Ok(CallToolResponse::InputRequired(required));
        }
        match outcome {
            Ok(result) => Ok(CallToolResponse::Complete(result)),
            Err(err) => err.into_call_result().map(CallToolResponse::Complete),
        }
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.prepare(&context).await?;
        let list = self.state.resource_list(&context);
        let (page, next_cursor) = self.page(list, request, &context)?;
        let mut result = ListResourcesResult::with_all_items(page);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.prepare(&context).await?;
        let list = self.state.resource_template_list(&context);
        let (page, next_cursor) = self.page(list, request, &context)?;
        let mut result = ListResourceTemplatesResult::with_all_items(page);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.prepare(&context).await?;
        let Some((route, variables)) = self.state.resource(request.uri.as_str()) else {
            // Unknown resource URI has its own JSON-RPC code per the spec.
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource: {}", request.uri),
                None,
            ));
        };

        let (call, rounds) = CallContext::new(
            context,
            self.session(),
            &self.endpoint,
            Input {
                method: "resources/read",
                target: request.uri.as_str(),
                arguments: None,
                responses: request.input_responses,
                state: request.request_state,
            },
        )?;

        let outcome = (route.invoke)(call.resource(request.uri, variables)).await;
        if let Some(required) = self.endpoint.input_required(rounds)? {
            return Ok(ReadResourceResponse::InputRequired(required));
        }
        match outcome {
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
        // rmcp acknowledges this synchronous filter before calling `listen`.
        // Accept URI interests independently of the initial catalog: the async
        // init hook may add resources. Delivery below checks the caller's view.
        Some(requested.supported_by(&self.endpoint.catalog.info.capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        self.prepare(context.request_context()).await?;
        let mut updates = self.endpoint.updates.subscribe();
        let mut list_changes = self.endpoint.sessions.subscribe_changes();
        loop {
            r2e_core::rt::select! {
                _ = context.cancelled() => return Ok(()),
                change = list_changes.recv() => {
                    let changed = match change {
                        Ok(changed) => changed,
                        Err(broadcast::error::RecvError::Lagged(_)) => Changed::ALL,
                        Err(broadcast::error::RecvError::Closed) => continue,
                    };
                    if !forward_list_changed(&context, changed).await {
                        return Ok(());
                    }
                },
                update = updates.recv() => match update {
                    Ok(uri) => {
                        if context.accepted().resource_subscriptions.as_ref()
                            .is_some_and(|uris| uris.contains(&uri))
                            && self.resource_visible(&uri, context.request_context())
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
        // Authorize and bind the SAME route: a concurrent replacement must
        // not inherit the authorization checked on the previous route.
        let caller = caller_in(context.extensions.get::<Parts>());
        let Some((route, _)) = self
            .state
            .resource(&request.uri)
            .filter(|(route, _)| visible_to(caller.as_ref(), &route.requirements))
        else {
            // Same answer for unknown and unauthorized URIs: no existence oracle.
            return Err(ErrorData::new(
                ErrorCode::RESOURCE_NOT_FOUND,
                format!("unknown resource: {}", request.uri),
                None,
            ));
        };
        self.legacy_subscriptions
            .write()
            .expect("MCP subscription lock poisoned")
            .insert(request.uri, Arc::downgrade(&route));

        if !self.legacy_forwarder_alive.swap(true, Ordering::AcqRel) {
            let alive = Arc::clone(&self.legacy_forwarder_alive);
            let subscriptions = Arc::downgrade(&self.legacy_subscriptions);
            let state = Arc::downgrade(&self.state);
            let updates = self.endpoint.updates.subscribe();
            let shutdown = self.endpoint.shutdown.clone();
            let peer = context.peer;
            r2e_core::rt::spawn_ctl(async move {
                forward_legacy_updates(updates, subscriptions, state, peer, shutdown).await;
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
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.prepare(&context).await?;
        let list = self.state.prompt_list(&context);
        let (page, next_cursor) = self.page(list, request, &context)?;
        let mut result = ListPromptsResult::with_all_items(page);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.prepare(&context).await?;
        let Some(route) = self.state.prompt(request.name.as_str()) else {
            // Unknown prompt name is invalid params per the MCP spec.
            return Err(ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                format!("unknown prompt: {}", request.name),
                None,
            ));
        };

        let (call, rounds) = CallContext::new(
            context,
            self.session(),
            &self.endpoint,
            Input {
                method: "prompts/get",
                target: request.name.as_str(),
                arguments: request.arguments.as_ref(),
                responses: request.input_responses,
                state: request.request_state,
            },
        )?;

        let outcome = (route.invoke)(call.prompt(request.arguments)).await;
        if let Some(required) = self.endpoint.input_required(rounds)? {
            return Ok(GetPromptResponse::InputRequired(required));
        }
        match outcome {
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
        // The resolved route outlives the borrows below.
        let (prompt_route, resource_route) = match &request.r#ref {
            Reference::Prompt(prompt) => (self.state.prompt(&prompt.name), None),
            Reference::Resource(resource) => (None, self.state.completion_resource(&resource.uri)),
            _ => (None, None),
        };
        let target = match &request.r#ref {
            Reference::Prompt(prompt) => prompt_route.as_ref().map(|route| {
                (
                    CompletionRef::Prompt(prompt.name.clone()),
                    &route.requirements,
                    &route.completions,
                )
            }),
            Reference::Resource(resource) => resource_route.as_ref().map(|route| {
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
        let caller = caller_in(context.extensions.get::<Parts>());
        let Some((reference, _, providers)) =
            target.filter(|(_, requirements, _)| visible_to(caller.as_ref(), requirements))
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
