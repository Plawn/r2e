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
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use r2e_core::http::Parts;
use r2e_core::rt::sync::{broadcast, OnceCell};
use rmcp::service::{Peer, RoleServer};

use crate::auth::McpPrincipal;
use crate::catalog::{scopes_allowed, Catalog, Changed, PrivateMembers, SessionView};
use crate::error::McpError;
use crate::route::{identity_in, PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

/// Why a session change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSessionError {
    /// The request has no MCP session to change: a sessionless client
    /// (protocol 2026-07-28+) or `mcp.stateless`. The change would be lost
    /// with the request.
    NoSession,
    /// No catalog member declares this group.
    UnknownGroup(String),
    /// A member with this key is already served by the catalog (in any
    /// group, hidden or not) or by this session.
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
    /// A session-private member declares OAuth scopes but `mcp.auth` is
    /// disabled — the boot-time check, at runtime.
    ScopesWithoutAuth {
        /// `"tool"`, `"resource"` or `"prompt"`.
        kind: &'static str,
        /// The member name / URI.
        key: String,
    },
}

impl fmt::Display for McpSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpSessionError::NoSession => f.write_str(
                "this client has no MCP session (sessionless protocol or stateless \
                 server): the session's member list cannot be changed",
            ),
            McpSessionError::UnknownGroup(group) => write!(f, "unknown MCP group `{group}`"),
            McpSessionError::Duplicate { kind, key } => {
                write!(f, "an MCP {kind} `{key}` already exists")
            }
            McpSessionError::InvalidTemplate { uri, reason } => {
                write!(f, "invalid MCP resource URI template `{uri}`: {reason}")
            }
            McpSessionError::ScopesWithoutAuth { kind, key } => write!(
                f,
                "MCP {kind} `{key}` declares OAuth scopes but `mcp.auth` is disabled"
            ),
        }
    }
}

impl std::error::Error for McpSessionError {}

/// Agent-actionable refusals (no session, name clash) become domain tool
/// failures the agent can read; programming errors are internal.
impl From<McpSessionError> for McpError {
    fn from(err: McpSessionError) -> Self {
        match err {
            McpSessionError::NoSession | McpSessionError::Duplicate { .. } => {
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
}

impl SessionInit {
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

/// The mutable state of one MCP session (one per rmcp session handler).
pub(crate) struct SessionState {
    catalog: Arc<Catalog>,
    view: RwLock<Arc<SessionView>>,
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
    pub(crate) fn new(catalog: Arc<Catalog>) -> Self {
        let view = RwLock::new(Arc::clone(&catalog.default_view));
        SessionState {
            catalog,
            view,
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

    pub(crate) fn view(&self) -> Arc<SessionView> {
        Arc::clone(&self.view.read().expect("MCP session view lock poisoned"))
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
        f: impl FnOnce(&mut Draft<'_>) -> Result<T, McpSessionError>,
    ) -> Result<(T, Changed), McpSessionError> {
        let _guard = self.write.lock().expect("MCP session write lock poisoned");
        let current = self.view();
        let mut draft = Draft {
            catalog: &self.catalog,
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
            .map_err(|reason| McpSessionError::Duplicate {
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

/// A mutable copy of a session view's inputs.
struct Draft<'a> {
    catalog: &'a Catalog,
    enabled: Vec<bool>,
    private: PrivateMembers,
}

impl Draft<'_> {
    fn set_group(&mut self, name: &str, on: bool) -> Result<(), McpSessionError> {
        let index = self
            .catalog
            .group(name)
            .ok_or_else(|| McpSessionError::UnknownGroup(name.to_owned()))?;
        self.enabled[index] = on;
        Ok(())
    }

    fn check_scopes(
        &self,
        kind: &'static str,
        key: &str,
        req: &crate::ToolRequirements,
    ) -> Result<(), McpSessionError> {
        if scopes_allowed(self.catalog.auth_enabled, req) {
            Ok(())
        } else {
            Err(McpSessionError::ScopesWithoutAuth {
                kind,
                key: key.to_owned(),
            })
        }
    }

    fn add_tool(&mut self, tool: ToolRoute) -> Result<(), McpSessionError> {
        self.check_scopes("tool", &tool.name, &tool.requirements)?;
        if self.catalog.has_tool_key(&tool.name)
            || self.private.tools.iter().any(|t| t.name == tool.name)
        {
            return Err(McpSessionError::Duplicate {
                kind: "tool",
                key: tool.name.into_owned(),
            });
        }
        self.private.tools.push(Arc::new(tool));
        Ok(())
    }

    fn add_resource(&mut self, resource: ResourceRoute) -> Result<(), McpSessionError> {
        self.check_scopes("resource", &resource.uri, &resource.requirements)?;
        if resource.is_template() {
            let shape = template_shape(&resource.uri)?;
            let clash = self.catalog.has_template_shape(&shape)
                || self
                    .private
                    .resources
                    .iter()
                    .filter(|r| r.is_template())
                    .any(|r| template_shape(&r.uri).is_ok_and(|s| s == shape));
            if clash {
                return Err(McpSessionError::Duplicate {
                    kind: "resource template",
                    key: resource.uri.into_owned(),
                });
            }
        } else if self.catalog.has_resource_key(&resource.uri)
            || self.private.resources.iter().any(|r| r.uri == resource.uri)
        {
            return Err(McpSessionError::Duplicate {
                kind: "resource",
                key: resource.uri.into_owned(),
            });
        }
        self.private.resources.push(Arc::new(resource));
        Ok(())
    }

    fn add_prompt(&mut self, prompt: PromptRoute) -> Result<(), McpSessionError> {
        self.check_scopes("prompt", &prompt.name, &prompt.requirements)?;
        if self.catalog.has_prompt_key(&prompt.name)
            || self.private.prompts.iter().any(|p| p.name == prompt.name)
        {
            return Err(McpSessionError::Duplicate {
                kind: "prompt",
                key: prompt.name.into_owned(),
            });
        }
        self.private.prompts.push(Arc::new(prompt));
        Ok(())
    }

    fn apply(&mut self, toolset: SessionToolset) -> Result<(), McpSessionError> {
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

fn template_shape(uri: &str) -> Result<String, McpSessionError> {
    UriTemplate::parse(uri)
        .map(|t| t.shape())
        .map_err(|reason| McpSessionError::InvalidTemplate {
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
/// [`McpSessionError::NoSession`].
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

    /// Whether `group` is currently enabled (`false` for an unknown group).
    pub fn is_group_enabled(&self, group: &str) -> bool {
        self.state
            .catalog()
            .group(group)
            .is_some_and(|index| self.state.view().enabled[index])
    }

    /// Whether the session currently serves a tool named `name` (before the
    /// per-caller auth filter).
    pub fn has_tool(&self, name: &str) -> bool {
        self.state.view().tools.route(name).is_some()
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
        f: impl FnOnce(&mut Draft<'_>) -> Result<T, McpSessionError>,
    ) -> Result<T, McpSessionError> {
        if !self.is_persistent() {
            return Err(McpSessionError::NoSession);
        }
        let (out, changed) = self.state.update(f)?;
        self.state.notify(changed);
        Ok(out)
    }

    /// Show a group's members to this session.
    pub fn enable_group(&self, group: &str) -> Result<(), McpSessionError> {
        self.change(|d| d.set_group(group, true))
    }

    /// Hide a group's members from this session.
    pub fn disable_group(&self, group: &str) -> Result<(), McpSessionError> {
        self.change(|d| d.set_group(group, false))
    }

    /// Add a session-private tool (see [`DynamicTool`](crate::DynamicTool)).
    /// Its name must not clash with any catalog tool — hidden groups
    /// included — nor with another private tool.
    pub fn add_tool(&self, tool: impl Into<ToolRoute>) -> Result<(), McpSessionError> {
        let tool = tool.into();
        self.change(|d| d.add_tool(tool))
    }

    /// Remove a session-private tool; `false` when none has that name.
    /// Catalog tools are hidden through their group instead.
    pub fn remove_tool(&self, name: &str) -> Result<bool, McpSessionError> {
        self.change(|d| Ok(remove_by(&mut d.private.tools, |t| t.name == name)))
    }

    /// Add a session-private resource (fixed URI or template).
    pub fn add_resource(&self, resource: impl Into<ResourceRoute>) -> Result<(), McpSessionError> {
        let resource = resource.into();
        self.change(|d| d.add_resource(resource))
    }

    /// Remove a session-private resource by its URI (or template text).
    pub fn remove_resource(&self, uri: &str) -> Result<bool, McpSessionError> {
        self.change(|d| Ok(remove_by(&mut d.private.resources, |r| r.uri == uri)))
    }

    /// Add a session-private prompt.
    pub fn add_prompt(&self, prompt: impl Into<PromptRoute>) -> Result<(), McpSessionError> {
        let prompt = prompt.into();
        self.change(|d| d.add_prompt(prompt))
    }

    /// Remove a session-private prompt; `false` when none has that name.
    pub fn remove_prompt(&self, name: &str) -> Result<bool, McpSessionError> {
        self.change(|d| Ok(remove_by(&mut d.private.prompts, |p| p.name == name)))
    }

    /// Apply a batch of changes atomically: on `Err` nothing changed.
    pub fn apply(&self, toolset: SessionToolset) -> Result<(), McpSessionError> {
        self.change(|d| d.apply(toolset))
    }
}

fn remove_by<T>(items: &mut Vec<Arc<T>>, matches: impl Fn(&T) -> bool) -> bool {
    let before = items.len();
    items.retain(|item| !matches(item));
    items.len() != before
}

struct SessionsInner {
    live: Mutex<Vec<Weak<SessionState>>>,
    /// Fan-out to sessionless `subscriptions/listen` streams.
    changed: broadcast::Sender<()>,
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

    pub(crate) fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.inner.changed.subscribe()
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
        for state in self.live() {
            state.notify(Changed::ALL);
        }
        let _ = self.inner.changed.send(());
    }
}
