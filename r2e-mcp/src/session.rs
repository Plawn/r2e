//! Per-session member lists: [`McpSession`] (reshape the list from inside a
//! member), [`McpSessionInit`] (shape it when a session opens) and
//! [`McpSessions`] (reach live sessions from anywhere else).
//!
//! A session serves the catalog members of its enabled groups plus its
//! session-private members. Hidden members answer exactly like unknown
//! ones — invisible means not callable — and the auth visibility filter
//! still applies on top.
//!
//! Changes are copy-on-write: a mutation clones the session's view, applies
//! the change, rebuilds the precomputed lists and swaps them in. In-flight
//! calls keep the route they resolved; the next request sees the new list.
//! Every change that alters a family's list sends that family's
//! `notifications/*/list_changed` to the session.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use r2e_core::http::middleware::Next;
use r2e_core::http::{Parts, Request, Response};
use r2e_core::rt::sync::{broadcast, OnceCell};
use rmcp::model::{Prompt, Resource, ResourceTemplate, Tool};
use rmcp::service::{Peer, RequestContext, RoleServer};

use crate::auth::McpPrincipal;
use crate::catalog::{
    prompt_completions_valid, resource_completions_valid, scopes_allowed, Catalog, Changed,
    PrivateMembers, SessionView,
};
use crate::error::McpError;
use crate::global::McpRegistry;
use crate::route::{identity_in, PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

/// Why a member change — on a session ([`McpSession`]) or on the global
/// registry ([`McpRegistry`](crate::McpRegistry)) — was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpMemberError {
    /// The request has no MCP session to change: a sessionless client
    /// (protocol 2026-07-28+) or `mcp.stateless`. The change would be lost
    /// with the request.
    NoSession,
    /// No catalog member declares this group.
    UnknownGroup(String),
    /// A member with this key is already served by the catalog (in any
    /// group, hidden or not) or by the same holder (this session, or the
    /// global registry).
    Duplicate {
        /// `"tool"`, `"resource"`, `"resource template"` or `"prompt"`.
        kind: &'static str,
        /// The clashing name / URI.
        key: String,
    },
    /// A session-private resource URI template does not parse.
    InvalidTemplate {
        /// The rejected template.
        uri: String,
        /// Parser diagnostic.
        reason: String,
    },
    /// A dynamic member declares OAuth scopes but `mcp.auth` is disabled —
    /// the boot-time check, at runtime.
    ScopesWithoutAuth {
        /// `"tool"`, `"resource"` or `"prompt"`.
        kind: &'static str,
        /// The member name / URI.
        key: String,
    },
    /// A dynamic member's completion provider names no argument (prompt) or
    /// template variable (resource) of the member, or names one twice.
    InvalidCompletion(String),
}

/// The former name of [`McpMemberError`], kept for source compatibility.
pub type McpSessionError = McpMemberError;

impl fmt::Display for McpMemberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpMemberError::NoSession => f.write_str(
                "this client has no MCP session (sessionless protocol or stateless \
                 server): the session's member list cannot be changed",
            ),
            McpMemberError::UnknownGroup(group) => write!(f, "unknown MCP group `{group}`"),
            McpMemberError::Duplicate { kind, key } => {
                write!(f, "an MCP {kind} `{key}` already exists")
            }
            McpMemberError::InvalidTemplate { uri, reason } => {
                write!(f, "invalid MCP resource URI template `{uri}`: {reason}")
            }
            McpMemberError::ScopesWithoutAuth { kind, key } => write!(
                f,
                "MCP {kind} `{key}` declares OAuth scopes but `mcp.auth` is disabled"
            ),
            McpMemberError::InvalidCompletion(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for McpMemberError {}

/// Agent-actionable refusals (no session, name clash) become domain tool
/// failures the agent can read; programming errors are internal.
impl From<McpMemberError> for McpError {
    fn from(err: McpMemberError) -> Self {
        match err {
            McpMemberError::NoSession | McpMemberError::Duplicate { .. } => {
                McpError::tool(err.to_string())
            }
            other => McpError::Internal(other.to_string()),
        }
    }
}

/// A batch of session changes, applied atomically (all or nothing).
///
/// Returned by [`McpSessionInit::init`]; also accepted by
/// [`McpSession::apply`].
#[derive(Default)]
#[must_use]
pub struct SessionToolset {
    enable: Vec<Cow<'static, str>>,
    disable: Vec<Cow<'static, str>>,
    tools: Vec<ToolRoute>,
    resources: Vec<ResourceRoute>,
    prompts: Vec<PromptRoute>,
}

impl SessionToolset {
    /// An empty change set (the session keeps the default list).
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable a group.
    pub fn enable(mut self, group: impl Into<Cow<'static, str>>) -> Self {
        self.enable.push(group.into());
        self
    }

    /// Enable a group when `condition` holds.
    pub fn enable_if(self, condition: bool, group: impl Into<Cow<'static, str>>) -> Self {
        if condition {
            self.enable(group)
        } else {
            self
        }
    }

    /// Disable a group (applied after every `enable`).
    pub fn disable(mut self, group: impl Into<Cow<'static, str>>) -> Self {
        self.disable.push(group.into());
        self
    }

    /// Add a session-private tool.
    pub fn tool(mut self, tool: impl Into<ToolRoute>) -> Self {
        self.tools.push(tool.into());
        self
    }

    /// Add a session-private resource (fixed URI or template).
    pub fn resource(mut self, resource: impl Into<ResourceRoute>) -> Self {
        self.resources.push(resource.into());
        self
    }

    /// Add a session-private prompt.
    pub fn prompt(mut self, prompt: impl Into<PromptRoute>) -> Self {
        self.prompts.push(prompt.into());
        self
    }
}

/// What a [`McpSessionInit`] hook sees about the session being opened.
pub struct SessionInit {
    parts: Option<Arc<Parts>>,
    persistent: bool,
    session_id: Option<SessionId>,
}

impl SessionInit {
    /// The transport session id, when already known — see [`SessionId`]
    /// for why it is `None` while the `initialize` handshake runs.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// The authenticated MCP caller, when `mcp.auth` is on.
    pub fn principal(&self) -> Option<&McpPrincipal> {
        self.extension_ref::<McpPrincipal>()
    }

    /// The caller's subject (`sub` claim), when authenticated.
    pub fn subject(&self) -> Option<&str> {
        self.principal().map(|p| p.user.sub.as_str())
    }

    /// Whether the authenticated caller holds `role`.
    pub fn has_role(&self, role: &str) -> bool {
        self.principal().is_some_and(|p| p.user.has_role(role))
    }

    /// Whether the caller's token carries `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.principal().is_some_and(|p| p.has_scope(scope))
    }

    /// Resolve a request-scoped identity — same semantics as
    /// [`ToolCall::identity`](crate::ToolCall::identity).
    pub fn identity<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        identity_in(self.parts.as_deref())
    }

    /// A request extension inserted by an HTTP layer.
    pub fn extension<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.extension_ref::<T>().cloned()
    }

    fn extension_ref<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.parts.as_ref()?.extensions.get::<T>()
    }

    /// A request header value (UTF-8 only).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.parts.as_ref()?.headers.get(name)?.to_str().ok()
    }

    /// The HTTP request head of the session's first request.
    pub fn parts(&self) -> Option<&Parts> {
        self.parts.as_deref()
    }

    /// Whether this is a real MCP session. `false` for sessionless clients
    /// and `mcp.stateless`, where the hook runs on EVERY request (keep it
    /// cheap, or cache by subject).
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }
}

/// Shape a session's member list when it opens.
///
/// Implemented by a bean and installed with
/// [`McpServer::session_init`](crate::McpServer::session_init). Runs once per
/// session, lazily on its first request (so the caller's identity is known);
/// an `Err` fails that request and is retried on the next one. For
/// sessionless clients it runs per request.
///
/// ```ignore
/// #[derive(Clone)]
/// struct Toolsets;
///
/// impl McpSessionInit for Toolsets {
///     async fn init(&self, s: &SessionInit) -> Result<SessionToolset, McpError> {
///         Ok(SessionToolset::new().enable_if(s.has_role("admin"), "admin"))
///     }
/// }
/// ```
pub trait McpSessionInit: Send + Sync + 'static {
    /// Compute the session's initial changes.
    fn init(
        &self,
        session: &SessionInit,
    ) -> impl Future<Output = Result<SessionToolset, McpError>> + Send;
}

type InitFuture = Pin<Box<dyn Future<Output = Result<SessionToolset, McpError>> + Send>>;

/// Type-erased [`McpSessionInit`] hook.
pub(crate) type SessionInitFn = Arc<dyn Fn(SessionInit) -> InitFuture + Send + Sync>;

pub(crate) fn erase_init<T: McpSessionInit + Clone>(hook: T) -> SessionInitFn {
    Arc::new(move |init: SessionInit| {
        let hook = hook.clone();
        Box::pin(async move { hook.init(&init).await })
    })
}

/// The transport-assigned identifier of an MCP session — the value of the
/// `Mcp-Session-Id` header under streamable HTTP.
///
/// Cheap to clone (a shared string). Read it from a call
/// ([`ToolCall::session_id`](crate::ToolCall::session_id)) or a handle
/// ([`McpSession::id`]), and look a session up with [`McpSessions::get`].
///
/// Known from the request *after* `initialize` on: the handshake itself
/// is the request that mints the id, so [`SessionInit::session_id`] is
/// `None` for an [`McpSessionInit`] hook running during `initialize`
/// (the id is attached as soon as rmcp's response leaves the server).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionId(Arc<str>);

impl SessionId {
    /// The id as sent on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SessionId {
    fn from(id: &str) -> Self {
        SessionId(Arc::from(id))
    }
}

impl From<String> for SessionId {
    fn from(id: String) -> Self {
        SessionId(Arc::from(id))
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for SessionId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for SessionId {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for SessionId {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

/// The mutable state of one MCP session (one per rmcp session handler).
pub(crate) struct SessionState {
    catalog: Arc<Catalog>,
    /// The endpoint-wide dynamic members, served behind the session's own.
    registry: McpRegistry,
    view: RwLock<Arc<SessionView>>,
    /// The transport session id, attached by [`track_session_id`] when the
    /// `initialize` response leaves (or by the handler from the request
    /// header, whichever runs first). Never set for a sessionless request.
    id: OnceLock<SessionId>,
    /// Serializes copy-on-write updates (readers never take it).
    write: Mutex<()>,
    /// Captured on the first request: the channel `list_changed`
    /// notifications travel on.
    peer: OnceLock<Peer<RoleServer>>,
    /// The principal subject the session is bound to (`None` = no auth).
    subject: OnceLock<Option<String>>,
    init: OnceCell<()>,
    /// Set by `initialize` under stateful serving: rmcp only delivers the
    /// handshake to a handler it created for a new (or restored) session.
    persistent: AtomicBool,
}

impl SessionState {
    pub(crate) fn new(catalog: Arc<Catalog>, registry: McpRegistry) -> Self {
        let view = RwLock::new(Arc::clone(&catalog.default_view));
        SessionState {
            catalog,
            registry,
            view,
            id: OnceLock::new(),
            write: Mutex::new(()),
            peer: OnceLock::new(),
            subject: OnceLock::new(),
            init: OnceCell::new(),
            persistent: AtomicBool::new(false),
        }
    }

    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Attach the transport session id (first writer wins; a repeat of the
    /// same id is a no-op).
    pub(crate) fn set_id(&self, id: &str) {
        if self.id.get().is_none() {
            let _ = self.id.set(SessionId::from(id));
        }
    }

    pub(crate) fn id(&self) -> Option<&SessionId> {
        self.id.get()
    }

    pub(crate) fn view(&self) -> Arc<SessionView> {
        Arc::clone(&self.view.read().expect("MCP session view lock poisoned"))
    }

    // ── Member resolution: the session's view first, then the global
    // registry. A session-private member shadows a global namesake; the
    // catalog never clashes with either (checked when they are added).

    /// The tool `name` resolves to for this session.
    pub(crate) fn tool(&self, name: &str) -> Option<Arc<ToolRoute>> {
        if let Some(route) = self.view().tools.route(name) {
            return Some(Arc::clone(route));
        }
        self.registry.view().tools.route(name).map(Arc::clone)
    }

    /// The resource `uri` resolves to (fixed URI, then template; session,
    /// then registry), with the template variables it captured.
    pub(crate) fn resource(
        &self,
        uri: &str,
    ) -> Option<(Arc<ResourceRoute>, BTreeMap<String, String>)> {
        if let Some((route, variables)) = self.view().resource_route(uri) {
            return Some((Arc::clone(route), variables));
        }
        self.registry
            .view()
            .resource_route(uri)
            .map(|(route, variables)| (Arc::clone(route), variables))
    }

    /// The prompt `name` resolves to for this session.
    pub(crate) fn prompt(&self, name: &str) -> Option<Arc<PromptRoute>> {
        if let Some(route) = self.view().prompts.route(name) {
            return Some(Arc::clone(route));
        }
        self.registry.view().prompts.route(name).map(Arc::clone)
    }

    /// The resource a `completion/complete` `ref/resource` names.
    pub(crate) fn completion_resource(&self, uri: &str) -> Option<Arc<ResourceRoute>> {
        if let Some(route) = self.view().completion_resource(uri) {
            return Some(Arc::clone(route));
        }
        self.registry
            .view()
            .completion_resource(uri)
            .map(Arc::clone)
    }

    // ── List payloads: the session's list followed by the registry's,
    // built into one allocation. With an empty registry (the common
    // deployment) this is exactly the prebuilt list's clone.

    pub(crate) fn tool_list(&self, context: &RequestContext<RoleServer>) -> Vec<Tool> {
        let view = self.view();
        let global = self.registry.view();
        if global.tools.is_empty() {
            return view.tools.visible_list(context);
        }
        let mut out = Vec::with_capacity(view.tools.len() + global.tools.len());
        view.tools.extend_visible(context, &mut out, |_| false);
        if view.private.tools.is_empty() {
            global.tools.extend_visible(context, &mut out, |_| false);
        } else {
            global
                .tools
                .extend_visible(context, &mut out, |name| view.tools.route(name).is_some());
        }
        out
    }

    pub(crate) fn resource_list(&self, context: &RequestContext<RoleServer>) -> Vec<Resource> {
        let view = self.view();
        let global = self.registry.view();
        if global.resources.is_empty() {
            return view.resources.visible_list(context);
        }
        let mut out = Vec::with_capacity(view.resources.len() + global.resources.len());
        view.resources.extend_visible(context, &mut out, |_| false);
        if view.private.resources.is_empty() {
            global
                .resources
                .extend_visible(context, &mut out, |_| false);
        } else {
            global
                .resources
                .extend_visible(context, &mut out, |uri| view.resources.route(uri).is_some());
        }
        out
    }

    pub(crate) fn resource_template_list(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Vec<ResourceTemplate> {
        let view = self.view();
        let global = self.registry.view();
        if global.resource_templates.is_empty() {
            return view.resource_templates.visible_list(context);
        }
        let mut out =
            Vec::with_capacity(view.resource_templates.len() + global.resource_templates.len());
        view.resource_templates
            .extend_visible(context, &mut out, |_| false);
        if view.private.resources.is_empty() {
            global
                .resource_templates
                .extend_visible(context, &mut out, |_| false);
        } else {
            global
                .resource_templates
                .extend_visible(context, &mut out, |raw| {
                    view.resource_templates.has_raw(raw)
                });
        }
        out
    }

    pub(crate) fn prompt_list(&self, context: &RequestContext<RoleServer>) -> Vec<Prompt> {
        let view = self.view();
        let global = self.registry.view();
        if global.prompts.is_empty() {
            return view.prompts.visible_list(context);
        }
        let mut out = Vec::with_capacity(view.prompts.len() + global.prompts.len());
        view.prompts.extend_visible(context, &mut out, |_| false);
        if view.private.prompts.is_empty() {
            global.prompts.extend_visible(context, &mut out, |_| false);
        } else {
            global
                .prompts
                .extend_visible(context, &mut out, |name| view.prompts.route(name).is_some());
        }
        out
    }

    pub(crate) fn capture_peer(&self, peer: &Peer<RoleServer>) {
        if self.peer.get().is_none() {
            let _ = self.peer.set(peer.clone());
        }
    }

    /// Bind the session to `subject` on first use; `false` when it is
    /// already bound to a different one (a replayed session id).
    pub(crate) fn bind(&self, subject: Option<&str>) -> bool {
        let bound = self.subject.get_or_init(|| subject.map(str::to_owned));
        bound.as_deref() == subject
    }

    pub(crate) fn subject(&self) -> Option<&str> {
        self.subject.get().and_then(|s| s.as_deref())
    }

    /// Whether this handler serves a real MCP session.
    pub(crate) fn is_persistent(&self) -> bool {
        self.persistent.load(Ordering::Acquire)
    }

    /// Called from `initialize`: under stateful serving the handler now owns
    /// a real session. `true` only the first time (register it then).
    pub(crate) fn open(&self) -> bool {
        self.catalog.stateful && !self.persistent.swap(true, Ordering::AcqRel)
    }

    /// Run the init hook once (retried after an error).
    pub(crate) async fn ensure_init(
        &self,
        hook: &SessionInitFn,
        parts: Option<&Parts>,
    ) -> Result<(), McpError> {
        self.init
            .get_or_try_init(|| async {
                let init = SessionInit {
                    parts: parts.cloned().map(Arc::new),
                    persistent: self.is_persistent(),
                    session_id: self.id().cloned(),
                };
                let toolset = hook(init).await?;
                // No notification: the session has not listed anything yet.
                self.update(|draft| draft.apply(toolset))
                    .map(|_| ())
                    .map_err(McpError::from)
            })
            .await
            .map(|_| ())
    }

    /// Copy-on-write update: `f` edits a draft of the current view; the new
    /// view replaces it only when `f` succeeds.
    fn update<T>(
        &self,
        f: impl FnOnce(&mut Draft<'_>) -> Result<T, McpMemberError>,
    ) -> Result<(T, Changed), McpMemberError> {
        let _guard = self.write.lock().expect("MCP session write lock poisoned");
        let current = self.view();
        let mut draft = Draft {
            catalog: Some(&self.catalog),
            enabled: current.enabled.to_vec(),
            private: current.private.clone(),
        };
        let out = f(&mut draft)?;
        if draft.private.same_as(&current.private) && draft.enabled[..] == current.enabled[..] {
            // A no-op (group already in that state, nothing removed): skip
            // the rebuild.
            return Ok((out, Changed::default()));
        }
        let next = self
            .catalog
            .view(draft.enabled.into_boxed_slice(), draft.private)
            .map_err(|reason| McpMemberError::Duplicate {
                kind: "member",
                key: reason,
            })?;
        let changed = current.diff(&next);
        if changed.any() {
            *self.view.write().expect("MCP session view lock poisoned") = Arc::new(next);
        }
        Ok((out, changed))
    }

    /// Send one `list_changed` per changed family to the session's peer, in
    /// the background (delivery must not hold the calling member).
    pub(crate) fn notify(&self, changed: Changed) {
        if !changed.any() {
            return;
        }
        let Some(peer) = self.peer.get().cloned() else {
            return;
        };
        r2e_core::rt::spawn_ctl(async move {
            if changed.tools {
                let _ = peer.notify_tool_list_changed().await;
            }
            if changed.resources {
                let _ = peer.notify_resource_list_changed().await;
            }
            if changed.prompts {
                let _ = peer.notify_prompt_list_changed().await;
            }
        });
    }
}

/// Deposited by the auth layer in the extensions of a request that carries
/// no `Mcp-Session-Id`: if it turns out to be an `initialize` that opens a
/// session, the handler hands the new session back through it, and the
/// layer keys it by the id rmcp puts in the response header.
#[derive(Clone, Default)]
pub(crate) struct SessionLink(Arc<Mutex<Option<Weak<SessionState>>>>);

impl SessionLink {
    pub(crate) fn set(&self, state: &Arc<SessionState>) {
        *self.0.lock().expect("MCP session link poisoned") = Some(Arc::downgrade(state));
    }

    pub(crate) fn take(&self) -> Option<Weak<SessionState>> {
        self.0.lock().expect("MCP session link poisoned").take()
    }

    fn peek(&self) -> Option<Arc<SessionState>> {
        self.0
            .lock()
            .expect("MCP session link poisoned")
            .as_ref()
            .and_then(Weak::upgrade)
    }
}

/// The streamable-HTTP session header (rmcp's `HEADER_SESSION_ID`).
pub(crate) const SESSION_ID_HEADER: &str = "mcp-session-id";

/// Innermost middleware on the MCP endpoint: when a request without an
/// `Mcp-Session-Id` turns out to open a session, attach the id rmcp minted
/// (response header) to the session the handler linked — so
/// [`McpSessions::get`] and [`McpSession::id`] work from the moment the
/// `initialize` response is on its way, not only from the client's next
/// request. Reuses the [`SessionLink`] the auth layer deposited, if any.
pub(crate) async fn track_session_id(mut req: Request, next: Next) -> Response {
    if req.headers().contains_key(SESSION_ID_HEADER) {
        return next.run(req).await;
    }
    let link = match req.extensions().get::<SessionLink>() {
        Some(link) => link.clone(),
        None => {
            let link = SessionLink::default();
            req.extensions_mut().insert(link.clone());
            link
        }
    };
    let response = next.run(req).await;
    if let Some(id) = response
        .headers()
        .get(SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        && let Some(state) = link.peek()
    {
        state.set_id(id);
    }
    response
}

/// `Mcp-Session-Id` → session, for the auth layer's session ↔ principal
/// check on every method (POST, the standalone SSE `GET`, `DELETE`).
///
/// Entries are weak: a session closed by `DELETE` or expired by rmcp drops
/// its handler, and the entry is pruned on the next insert.
#[derive(Clone, Default)]
pub(crate) struct SessionBindings(Arc<Mutex<HashMap<Box<str>, Weak<SessionState>>>>);

impl SessionBindings {
    pub(crate) fn insert(&self, id: &str, state: Weak<SessionState>) {
        let mut map = self.0.lock().expect("MCP session bindings poisoned");
        map.retain(|_, s| s.strong_count() > 0);
        map.insert(id.into(), state);
    }

    /// Whether `subject` may use the session `id`. `false` only when a live
    /// session with that id is bound to another subject; an unknown id
    /// passes (rmcp answers 404, or the handler binds a restored session).
    pub(crate) fn admits(&self, id: &str, subject: &str) -> bool {
        let state = self
            .0
            .lock()
            .expect("MCP session bindings poisoned")
            .get(id)
            .and_then(Weak::upgrade);
        match state.as_deref().and_then(SessionState::subject) {
            Some(bound) => bound == subject,
            None => true,
        }
    }
}

/// A mutable copy of a view's inputs: a session's (its enabled groups and
/// private members, validated against the catalog) or the global registry's
/// (members only; `catalog` is `None` until the endpoint is mounted, and the
/// catalog checks run then).
pub(crate) struct Draft<'a> {
    pub(crate) catalog: Option<&'a Catalog>,
    pub(crate) enabled: Vec<bool>,
    pub(crate) private: PrivateMembers,
}

impl Draft<'_> {
    fn set_group(&mut self, name: &str, on: bool) -> Result<(), McpMemberError> {
        let index = self
            .catalog
            .and_then(|catalog| catalog.group(name))
            .ok_or_else(|| McpMemberError::UnknownGroup(name.to_owned()))?;
        self.enabled[index] = on;
        Ok(())
    }

    fn check_scopes(
        &self,
        kind: &'static str,
        key: &str,
        req: &crate::ToolRequirements,
    ) -> Result<(), McpMemberError> {
        let allowed = self
            .catalog
            .is_none_or(|catalog| scopes_allowed(catalog.auth_enabled, req));
        if allowed {
            Ok(())
        } else {
            Err(McpMemberError::ScopesWithoutAuth {
                kind,
                key: key.to_owned(),
            })
        }
    }

    pub(crate) fn add_tool(&mut self, tool: ToolRoute) -> Result<(), McpMemberError> {
        self.check_scopes("tool", &tool.name, &tool.requirements)?;
        if self
            .catalog
            .is_some_and(|catalog| catalog.has_tool_key(&tool.name))
            || self.private.tools.iter().any(|t| t.name == tool.name)
        {
            return Err(McpMemberError::Duplicate {
                kind: "tool",
                key: tool.name.into_owned(),
            });
        }
        self.private.tools.push(Arc::new(tool));
        Ok(())
    }

    pub(crate) fn add_resource(&mut self, resource: ResourceRoute) -> Result<(), McpMemberError> {
        self.check_scopes("resource", &resource.uri, &resource.requirements)?;
        resource_completions_valid(&resource).map_err(McpMemberError::InvalidCompletion)?;
        if resource.is_template() {
            let shape = template_shape(&resource.uri)?;
            let clash = self
                .catalog
                .is_some_and(|catalog| catalog.has_template_shape(&shape))
                || self
                    .private
                    .resources
                    .iter()
                    .filter(|r| r.is_template())
                    .any(|r| template_shape(&r.uri).is_ok_and(|s| s == shape));
            if clash {
                return Err(McpMemberError::Duplicate {
                    kind: "resource template",
                    key: resource.uri.into_owned(),
                });
            }
        } else if self
            .catalog
            .is_some_and(|catalog| catalog.has_resource_key(&resource.uri))
            || self.private.resources.iter().any(|r| r.uri == resource.uri)
        {
            return Err(McpMemberError::Duplicate {
                kind: "resource",
                key: resource.uri.into_owned(),
            });
        }
        self.private.resources.push(Arc::new(resource));
        Ok(())
    }

    pub(crate) fn add_prompt(&mut self, prompt: PromptRoute) -> Result<(), McpMemberError> {
        self.check_scopes("prompt", &prompt.name, &prompt.requirements)?;
        prompt_completions_valid(&prompt).map_err(McpMemberError::InvalidCompletion)?;
        if self
            .catalog
            .is_some_and(|catalog| catalog.has_prompt_key(&prompt.name))
            || self.private.prompts.iter().any(|p| p.name == prompt.name)
        {
            return Err(McpMemberError::Duplicate {
                kind: "prompt",
                key: prompt.name.into_owned(),
            });
        }
        self.private.prompts.push(Arc::new(prompt));
        Ok(())
    }

    fn apply(&mut self, toolset: SessionToolset) -> Result<(), McpMemberError> {
        for group in &toolset.enable {
            self.set_group(group, true)?;
        }
        for group in &toolset.disable {
            self.set_group(group, false)?;
        }
        for tool in toolset.tools {
            self.add_tool(tool)?;
        }
        for resource in toolset.resources {
            self.add_resource(resource)?;
        }
        for prompt in toolset.prompts {
            self.add_prompt(prompt)?;
        }
        Ok(())
    }
}

fn template_shape(uri: &str) -> Result<String, McpMemberError> {
    UriTemplate::parse(uri)
        .map(|t| t.shape())
        .map_err(|reason| McpMemberError::InvalidTemplate {
            uri: uri.to_owned(),
            reason,
        })
}

/// A handle on the MCP session serving the current request.
///
/// Declare it as a member parameter (`session: McpSession`) — the MCP peer
/// of `CancelToken` — or reach live sessions through [`McpSessions`]. Every
/// change applies to this session only and notifies its client with
/// `notifications/*/list_changed`.
///
/// ```ignore
/// #[tool]
/// async fn enable_git(&self, session: McpSession) -> Result<&'static str, McpError> {
///     session.enable_group("git")?;
///     Ok("git tools enabled")
/// }
/// ```
///
/// Only a real MCP session can change: for a sessionless client (protocol
/// 2026-07-28+) the handle is read-only and every change returns
/// [`McpMemberError::NoSession`].
#[derive(Clone)]
pub struct McpSession {
    state: Arc<SessionState>,
}

impl fmt::Debug for McpSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpSession")
            .field("persistent", &self.is_persistent())
            .field("subject", &self.state.subject())
            .finish_non_exhaustive()
    }
}

impl McpSession {
    pub(crate) fn new(state: Arc<SessionState>) -> Self {
        McpSession { state }
    }

    /// Whether changes persist beyond the current request.
    pub fn is_persistent(&self) -> bool {
        self.state.is_persistent()
    }

    /// The principal subject the session is bound to, when authenticated.
    pub fn subject(&self) -> Option<&str> {
        self.state.subject()
    }

    /// The transport session id (`Mcp-Session-Id`); `None` for a
    /// sessionless request or while the `initialize` handshake runs.
    pub fn id(&self) -> Option<&SessionId> {
        self.state.id()
    }

    /// Whether `group` is currently enabled (`false` for an unknown group).
    pub fn is_group_enabled(&self, group: &str) -> bool {
        self.state
            .catalog()
            .group(group)
            .is_some_and(|index| self.state.view().enabled[index])
    }

    /// Whether the session currently serves a tool named `name` — its own
    /// or a global one (before the per-caller auth filter).
    pub fn has_tool(&self, name: &str) -> bool {
        self.state.tool(name).is_some()
    }

    /// The groups declared by the catalog, in declaration order.
    pub fn groups(&self) -> Vec<String> {
        self.state
            .catalog()
            .group_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn change<T>(
        &self,
        f: impl FnOnce(&mut Draft<'_>) -> Result<T, McpMemberError>,
    ) -> Result<T, McpMemberError> {
        if !self.is_persistent() {
            return Err(McpMemberError::NoSession);
        }
        let (out, changed) = self.state.update(f)?;
        self.state.notify(changed);
        Ok(out)
    }

    /// Show a group's members to this session.
    pub fn enable_group(&self, group: &str) -> Result<(), McpMemberError> {
        self.change(|d| d.set_group(group, true))
    }

    /// Hide a group's members from this session.
    pub fn disable_group(&self, group: &str) -> Result<(), McpMemberError> {
        self.change(|d| d.set_group(group, false))
    }

    /// Add a session-private tool (see [`DynamicTool`](crate::DynamicTool)).
    /// Its name must not clash with any catalog tool — hidden groups
    /// included — nor with another private tool.
    pub fn add_tool(&self, tool: impl Into<ToolRoute>) -> Result<(), McpMemberError> {
        let tool = tool.into();
        self.change(|d| d.add_tool(tool))
    }

    /// Remove a session-private tool; `false` when none has that name.
    /// Catalog tools are hidden through their group instead.
    pub fn remove_tool(&self, name: &str) -> Result<bool, McpMemberError> {
        self.change(|d| Ok(remove_by(&mut d.private.tools, |t| t.name == name)))
    }

    /// Add a session-private resource (fixed URI or template).
    pub fn add_resource(&self, resource: impl Into<ResourceRoute>) -> Result<(), McpMemberError> {
        let resource = resource.into();
        self.change(|d| d.add_resource(resource))
    }

    /// Remove a session-private resource by its URI (or template text).
    pub fn remove_resource(&self, uri: &str) -> Result<bool, McpMemberError> {
        self.change(|d| Ok(remove_by(&mut d.private.resources, |r| r.uri == uri)))
    }

    /// Add a session-private prompt.
    pub fn add_prompt(&self, prompt: impl Into<PromptRoute>) -> Result<(), McpMemberError> {
        let prompt = prompt.into();
        self.change(|d| d.add_prompt(prompt))
    }

    /// Remove a session-private prompt; `false` when none has that name.
    pub fn remove_prompt(&self, name: &str) -> Result<bool, McpMemberError> {
        self.change(|d| Ok(remove_by(&mut d.private.prompts, |p| p.name == name)))
    }

    /// Apply a batch of changes atomically: on `Err` nothing changed.
    pub fn apply(&self, toolset: SessionToolset) -> Result<(), McpMemberError> {
        self.change(|d| d.apply(toolset))
    }
}

pub(crate) fn remove_by<T>(items: &mut Vec<Arc<T>>, matches: impl Fn(&T) -> bool) -> bool {
    let before = items.len();
    items.retain(|item| !matches(item));
    items.len() != before
}

struct SessionsInner {
    live: Mutex<Vec<Weak<SessionState>>>,
    /// Fan-out to sessionless `subscriptions/listen` streams.
    changed: broadcast::Sender<Changed>,
}

/// Every live MCP session of the endpoint — a bean provided by the
/// [`McpServer`](crate::McpServer) plugin.
///
/// For changes decided outside a member: an admin endpoint, a
/// `#[consumer]` reacting to a permission change, a scheduled cleanup.
///
/// ```ignore
/// for session in sessions.for_subject(&user_id) {
///     session.disable_group("billing")?;
/// }
/// ```
///
/// Only real sessions (stateful serving) are listed, from their
/// `initialize` handshake on.
/// Sessions are held weakly: closing one drops it from the registry.
#[derive(Clone)]
pub struct McpSessions {
    inner: Arc<SessionsInner>,
}

impl Default for McpSessions {
    fn default() -> Self {
        let (changed, _) = broadcast::channel(16);
        McpSessions {
            inner: Arc::new(SessionsInner {
                live: Mutex::new(Vec::new()),
                changed,
            }),
        }
    }
}

impl McpSessions {
    pub(crate) fn register(&self, state: &Arc<SessionState>) {
        let mut live = self
            .inner
            .live
            .lock()
            .expect("MCP session registry poisoned");
        live.retain(|s| s.strong_count() > 0);
        live.push(Arc::downgrade(state));
    }

    pub(crate) fn subscribe_changes(&self) -> broadcast::Receiver<Changed> {
        self.inner.changed.subscribe()
    }

    /// Tell every client — live sessions and sessionless
    /// `subscriptions/listen` streams — to re-list the `changed` families.
    pub(crate) fn notify(&self, changed: Changed) {
        if !changed.any() {
            return;
        }
        for state in self.live() {
            state.notify(changed);
        }
        let _ = self.inner.changed.send(changed);
    }

    fn live(&self) -> Vec<Arc<SessionState>> {
        let mut live = self
            .inner
            .live
            .lock()
            .expect("MCP session registry poisoned");
        live.retain(|s| s.strong_count() > 0);
        live.iter().filter_map(Weak::upgrade).collect()
    }

    /// Every live session.
    pub fn all(&self) -> Vec<McpSession> {
        self.live().into_iter().map(McpSession::new).collect()
    }

    /// The live session with the transport id `id`, if any.
    pub fn get(&self, id: &str) -> Option<McpSession> {
        self.live()
            .into_iter()
            .find(|state| state.id().is_some_and(|known| known == id))
            .map(McpSession::new)
    }

    /// The live sessions bound to `subject` (the principal's `sub`).
    pub fn for_subject(&self, subject: &str) -> Vec<McpSession> {
        self.live()
            .into_iter()
            .filter(|state| state.subject() == Some(subject))
            .map(McpSession::new)
            .collect()
    }

    /// The number of live sessions.
    pub fn len(&self) -> usize {
        self.live().len()
    }

    /// Whether no session is live.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tell every client to re-list tools, resources and prompts — after a
    /// change that alters what [`McpSessionInit`] or the auth filter would
    /// compute (e.g. a role granted). Also reaches sessionless clients
    /// holding a `subscriptions/listen` stream.
    pub fn notify_list_changed(&self) {
        self.notify(Changed::ALL);
    }
}
