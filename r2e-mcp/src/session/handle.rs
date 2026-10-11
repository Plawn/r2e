use std::fmt;
use std::sync::Arc;

use super::error::McpMemberError;
use super::id::SessionId;
use super::init::SessionToolset;
use super::state::{remove_by, Draft, SessionState};
use crate::route::{PromptRoute, ResourceRoute, ToolRoute};

/// A handle on the MCP session serving the current request.
///
/// Declare it as a member parameter (`session: McpSession`) — the MCP peer
/// of `CancelToken` — or reach live sessions through [`McpSessions`](super::McpSessions). Every
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
