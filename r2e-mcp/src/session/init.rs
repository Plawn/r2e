use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use r2e_core::http::Parts;

use super::id::SessionId;
use crate::auth::McpPrincipal;
use crate::error::McpError;
use crate::route::{identity_in, PromptRoute, ResourceRoute, ToolRoute};

/// A batch of session changes, applied atomically (all or nothing).
///
/// Returned by [`McpSessionInit::init`]; also accepted by
/// [`McpSession::apply`](super::McpSession::apply).
#[derive(Default)]
#[must_use]
pub struct SessionToolset {
    pub(super) enable: Vec<Cow<'static, str>>,
    pub(super) disable: Vec<Cow<'static, str>>,
    pub(super) tools: Vec<ToolRoute>,
    pub(super) resources: Vec<ResourceRoute>,
    pub(super) prompts: Vec<PromptRoute>,
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
    pub(super) parts: Option<Arc<Parts>>,
    pub(super) persistent: bool,
    pub(super) session_id: Option<SessionId>,
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
