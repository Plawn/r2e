use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use r2e_core::http::Parts;
use r2e_core::rt;
use r2e_core::rt::sync::OnceCell;
use rmcp::model::InitializeRequestParams;
use rmcp::service::{Peer, RoleServer};

use super::access::{Access, ViewFilterFn};
use super::error::McpMemberError;
use super::id::SessionId;
use super::init::{SessionInit, SessionInitFn, SessionToolset};
use crate::catalog::{
    prompt_completions_valid, resource_completions_valid, scopes_allowed, Catalog, CatalogKeys,
    Changed, PrivateMembers, SessionView,
};
use crate::error::McpError;
use crate::global::McpRegistry;
use crate::route::{McpGroup, PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

/// The mutable state of one MCP session (one per rmcp session handler).
pub(crate) struct SessionState {
    catalog: Arc<Catalog>,
    /// The endpoint-wide dynamic members, served behind the session's own.
    registry: McpRegistry,
    view: RwLock<Arc<SessionView>>,
    /// The transport session id, attached by [`TrackSessionId`](super::TrackSessionId) when the
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
    /// Set once the transport connection behind this state is gone (a
    /// loopback session closed, dropped or cancelled mid-handshake). The
    /// registries treat a closed state as absent even while an
    /// [`McpSession`](super::McpSession) handle keeps the allocation alive,
    /// so the id is reusable the moment the connection ends.
    closed: AtomicBool,
    /// Set when rmcp rebuilt this session from an [`McpSessionStore`](super::McpSessionStore)
    /// record instead of a client `initialize`.
    restored: AtomicBool,
    /// Set once `initialize` succeeded (negotiation included): only then is
    /// the session worth a store record — rmcp persists a session whatever
    /// the handshake answered.
    initialized: AtomicBool,
    /// When the session record was last written (or scheduled); `None`
    /// until the handshake is persisted. Drives the half-life re-arm.
    last_persisted: Mutex<Option<rt::Instant>>,
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
            closed: AtomicBool::new(false),
            restored: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            last_persisted: Mutex::new(None),
        }
    }

    pub(crate) fn mark_restored(&self) {
        self.restored.store(true, Ordering::Release);
    }

    pub(crate) fn is_restored(&self) -> bool {
        self.restored.load(Ordering::Acquire)
    }

    pub(crate) fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Record that the session record is being written now.
    pub(crate) fn mark_persisted(&self) {
        *self
            .last_persisted
            .lock()
            .expect("MCP session persistence clock poisoned") = Some(rt::Instant::now());
    }

    /// Time since the record was last written; `None` when never.
    pub(crate) fn persisted_since(&self) -> Option<Duration> {
        self.last_persisted
            .lock()
            .expect("MCP session persistence clock poisoned")
            .map(|at| at.elapsed())
    }

    /// The client's handshake parameters, once `initialize` stored them on
    /// the peer.
    pub(crate) fn peer_info(&self) -> Option<Arc<InitializeRequestParams>> {
        self.peer.get().and_then(|peer| peer.peer_info())
    }

    /// Mark the connection behind this state as ended (idempotent). The
    /// session registries stop listing it and its id becomes claimable.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Whether [`close`](Self::close) ran.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
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

    pub(crate) fn registry_view(&self) -> Arc<SessionView> {
        self.registry.view()
    }

    /// This request's window on the members — see [`Access`]. `parts` is
    /// the request head (`None` outside a request: no caller, no
    /// extensions); `filter` the endpoint's view filter.
    pub(crate) fn access<'a>(
        &'a self,
        parts: Option<&'a Parts>,
        filter: Option<&'a ViewFilterFn>,
    ) -> Access<'a> {
        Access::new(self, self.view(), self.registry.view(), parts, filter)
    }

    /// Whether the group `name` is on for the session `view` belongs to:
    /// a catalog group, or one a dynamic member (session-private, then
    /// `registry`) declares. `false` for an unknown name.
    pub(crate) fn group_enabled(
        &self,
        view: &SessionView,
        registry: &SessionView,
        name: &str,
    ) -> bool {
        if let Some(index) = self.catalog.group(name) {
            return view.enabled[index];
        }
        view.private
            .group(name)
            .or_else(|| registry.private.group(name))
            .is_some_and(|group| view.group_on(self.catalog.keys(), group))
    }

    /// Every group this session can switch: the catalog's, then the ones
    /// its private members and the registry's declare.
    pub(crate) fn group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .catalog
            .group_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (view, registry) = (self.view(), self.registry.view());
        for group in view.private.groups().chain(registry.private.groups()) {
            if !names.iter().any(|known| *known == group.name) {
                names.push(group.name.to_string());
            }
        }
        names
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
                    restored: self.is_restored(),
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
        let registry = self.registry.view();
        let mut draft = Draft {
            catalog: Some(self.catalog.keys()),
            enabled: current.enabled.to_vec(),
            dynamic: current.dynamic.clone(),
            private: current.private.clone(),
            registry: Some(&registry.private),
        };
        let out = f(&mut draft)?;
        if draft.private.same_as(&current.private)
            && draft.enabled[..] == current.enabled[..]
            && draft.dynamic == current.dynamic
        {
            // A no-op (group already in that state, nothing removed): skip
            // the rebuild.
            return Ok((out, Changed::default()));
        }
        let next = self
            .catalog
            .view(
                draft.enabled.into_boxed_slice(),
                draft.dynamic,
                draft.private,
            )
            .map_err(|reason| McpMemberError::Duplicate {
                kind: "member",
                key: reason,
            })?;
        // The session's own families, plus the registry's members whose
        // group this change switched (they are gated at request time, so
        // no family differs for them).
        let changed = current
            .diff(&next)
            .or(current.registry_diff(&next, self.catalog.keys(), &registry));
        *self.view.write().expect("MCP session view lock poisoned") = Arc::new(next);
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
    pub(crate) catalog: Option<&'a CatalogKeys>,
    pub(crate) enabled: Vec<bool>,
    /// Explicit states of the groups only dynamic members declare.
    pub(crate) dynamic: BTreeMap<String, bool>,
    pub(crate) private: PrivateMembers,
    /// The global registry's members, for a session's draft: their groups
    /// are switchable by the session. `None` for the registry's own draft.
    pub(crate) registry: Option<&'a PrivateMembers>,
}

impl Draft<'_> {
    /// Switch a group: a catalog group, or one a dynamic member currently
    /// declares (the draft's own, or the registry's) — any other name is
    /// [`McpMemberError::UnknownGroup`].
    pub(super) fn set_group(&mut self, name: &str, on: bool) -> Result<(), McpMemberError> {
        if let Some(index) = self.catalog.and_then(|catalog| catalog.group(name)) {
            self.enabled[index] = on;
            return Ok(());
        }
        let declared = self.private.group(name).is_some()
            || self
                .registry
                .is_some_and(|registry| registry.group(name).is_some());
        if !declared {
            return Err(McpMemberError::UnknownGroup(name.to_owned()));
        }
        self.dynamic.insert(name.to_owned(), on);
        Ok(())
    }

    /// A dynamic member's group must agree on `opt_in` with the catalog
    /// group of that name, with the members already in this holder and —
    /// for a session — with the registry's.
    fn check_group(&self, group: &Option<McpGroup>) -> Result<(), McpMemberError> {
        let Some(group) = group else {
            return Ok(());
        };
        let declared = self
            .catalog
            .and_then(|catalog| catalog.group_opt_in(&group.name))
            .or_else(|| self.private.group(&group.name).map(|g| g.opt_in))
            .or_else(|| {
                let registry = self.registry?.group(&group.name)?;
                Some(registry.opt_in)
            });
        match declared {
            Some(opt_in) if opt_in != group.opt_in => {
                Err(McpMemberError::GroupMismatch(group.name.to_string()))
            }
            _ => Ok(()),
        }
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
        self.check_group(&tool.group)?;
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
        self.check_group(&resource.group)?;
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
        self.check_group(&prompt.group)?;
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
        // Members first: a group the toolset's own members declare is
        // switchable in the same batch.
        for tool in toolset.tools {
            self.add_tool(tool)?;
        }
        for resource in toolset.resources {
            self.add_resource(resource)?;
        }
        for prompt in toolset.prompts {
            self.add_prompt(prompt)?;
        }
        for group in &toolset.enable {
            self.set_group(group, true)?;
        }
        for group in &toolset.disable {
            self.set_group(group, false)?;
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
