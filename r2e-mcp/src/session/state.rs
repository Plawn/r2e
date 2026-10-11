use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use r2e_core::http::Parts;
use r2e_core::rt::sync::OnceCell;
use rmcp::model::{Prompt, Resource, ResourceTemplate, Tool};
use rmcp::service::{Peer, RequestContext, RoleServer};

use super::error::McpMemberError;
use super::id::SessionId;
use super::init::{SessionInit, SessionInitFn, SessionToolset};
use crate::catalog::{
    prompt_completions_valid, resource_completions_valid, scopes_allowed, Catalog, Changed,
    PrivateMembers, SessionView,
};
use crate::error::McpError;
use crate::global::McpRegistry;
use crate::route::{PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

/// The mutable state of one MCP session (one per rmcp session handler).
pub(crate) struct SessionState {
    catalog: Arc<Catalog>,
    /// The endpoint-wide dynamic members, served behind the session's own.
    registry: McpRegistry,
    view: RwLock<Arc<SessionView>>,
    /// The transport session id, attached by [`track_session_id`](super::track_session_id) when the
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
    pub(super) fn update<T>(
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
    pub(super) fn set_group(&mut self, name: &str, on: bool) -> Result<(), McpMemberError> {
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

    pub(super) fn apply(&mut self, toolset: SessionToolset) -> Result<(), McpMemberError> {
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

pub(crate) fn remove_by<T>(items: &mut Vec<Arc<T>>, matches: impl Fn(&T) -> bool) -> bool {
    let before = items.len();
    items.retain(|item| !matches(item));
    items.len() != before
}
