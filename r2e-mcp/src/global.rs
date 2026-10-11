//! The endpoint-wide dynamic member registry ([`McpRegistry`]).
//!
//! Three member holders serve one MCP endpoint, in this precedence:
//!
//! 1. the **catalog** — every `#[mcp_routes]` member registered at boot
//!    (immutable; grouped; see [`Catalog`]);
//! 2. a session's **private members** ([`McpSession::add_tool`] & co.) —
//!    visible to that session only;
//! 3. the **global registry** — this module: members any bean can add or
//!    remove after boot, visible to every session and to sessionless
//!    callers.
//!
//! A catalog key can never be taken by a dynamic member (the add fails, or
//! the boot panics when the member was added before the endpoint was
//! mounted). A session-private member and a global one may share a key:
//! the session's wins for that session (resolution and lists), with no
//! cross-check at add time — the two holders are owned by different code
//! and must not be able to break each other.
//!
//! The registry keeps one precomputed [`SessionView`] (the same type a
//! session uses, with no catalog groups behind it), rebuilt on every change
//! and read lock-free-ish (`RwLock` read + `Arc` clone) on every request.
//! Session views are NOT rebuilt when the registry changes: the handler
//! merges the two at request time, so a global change costs O(global
//! members) once, not O(sessions × members).

use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::catalog::{Catalog, PrivateMembers, SessionView};
use crate::route::{PromptRoute, ResourceRoute, ToolRoute};
use crate::session::{remove_by, Draft, McpMemberError, McpSessions};

/// Label the registry's members carry in duplicate diagnostics.
const LABEL: &str = "<registry>";

/// The endpoint-wide dynamic member registry — a bean provided by
/// [`McpServer`](crate::McpServer): inject it anywhere (a consumer, an admin
/// controller, a `#[post_construct]`) to add or remove tools, resources and
/// prompts every MCP session serves.
///
/// Every change is atomic — [`edit`](Self::edit) applies several at once —
/// and broadcasts the matching `notifications/*/list_changed` to every live
/// session and sessionless `subscriptions/listen` stream, with no per-session
/// work: sessions read the registry's view at request time.
///
/// ```rust,ignore
/// #[derive(Clone)]
/// pub struct PluginLoader {
///     #[inject] mcp: McpRegistry,
/// }
///
/// impl PluginLoader {
///     async fn load(&self, plugin: &Plugin) -> Result<(), McpMemberError> {
///         self.mcp.edit(|edit| {
///             for tool in plugin.tools() {
///                 edit.upsert_tool(tool)?;
///             }
///             Ok(())
///         })
///     }
/// }
/// ```
///
/// Members added before the endpoint is mounted (at build time, from a
/// `#[post_construct]` or `#[on_start]`) are validated against the catalog
/// when it is: a key clash panics at boot like any other catalog conflict.
/// With `mcp.enabled = false` the registry accepts members and serves
/// nothing.
#[derive(Clone, Default)]
pub struct McpRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    view: RwLock<Arc<SessionView>>,
    /// Serializes edits (the view swap itself is a single write).
    write: Mutex<()>,
    /// Set when the endpoint is mounted ([`attach`](McpRegistry::attach)).
    catalog: OnceLock<Arc<Catalog>>,
    sessions: OnceLock<McpSessions>,
}

impl Default for RegistryInner {
    fn default() -> Self {
        let view = SessionView::standalone(LABEL, false, PrivateMembers::default())
            .expect("an empty member set has no duplicates");
        RegistryInner {
            view: RwLock::new(Arc::new(view)),
            write: Mutex::new(()),
            catalog: OnceLock::new(),
            sessions: OnceLock::new(),
        }
    }
}

impl McpRegistry {
    /// The current global view (one `Arc` clone).
    pub(crate) fn view(&self) -> Arc<SessionView> {
        Arc::clone(
            &self
                .inner
                .view
                .read()
                .expect("MCP registry view lock poisoned"),
        )
    }

    /// Bind the registry to the mounted endpoint: from now on every add is
    /// checked against the catalog and every change notifies `sessions`.
    /// Members added earlier are re-validated here.
    ///
    /// # Panics
    ///
    /// When a member added before the mount clashes with the catalog (same
    /// key, or same resource-template shape), or declares scopes while
    /// `mcp.auth` is off — the boot-time checks every catalog member gets.
    pub(crate) fn attach(&self, catalog: Arc<Catalog>, sessions: McpSessions) {
        let _guard = self
            .inner
            .write
            .lock()
            .expect("MCP registry write lock poisoned");
        if self.inner.catalog.set(Arc::clone(&catalog)).is_err() {
            panic!("McpRegistry is already bound to an MCP endpoint");
        }
        let _ = self.inner.sessions.set(sessions);
        let current = self.view();
        let mut draft = Draft {
            catalog: Some(&catalog),
            enabled: Vec::new(),
            private: PrivateMembers::default(),
        };
        let revalidate =
            |err: McpMemberError| panic!("MCP registry member added before boot is invalid: {err}");
        for tool in &current.private.tools {
            draft
                .add_tool(ToolRoute::clone(tool))
                .unwrap_or_else(revalidate);
        }
        for resource in &current.private.resources {
            draft
                .add_resource(ResourceRoute::clone(resource))
                .unwrap_or_else(revalidate);
        }
        for prompt in &current.private.prompts {
            draft
                .add_prompt(PromptRoute::clone(prompt))
                .unwrap_or_else(revalidate);
        }
        // Keep the route objects already handed out (session shadowing and
        // legacy subscriptions compare by identity).
        let next = SessionView::standalone(LABEL, catalog.filter(), current.private.clone())
            .unwrap_or_else(|key| panic!("MCP registry: duplicate member `{key}`"));
        *self
            .inner
            .view
            .write()
            .expect("MCP registry view lock poisoned") = Arc::new(next);
    }

    /// Apply several changes atomically: either every step of `f` applies —
    /// and ONE `list_changed` per touched family goes out — or none does.
    ///
    /// `f` runs under the registry's write lock: keep it synchronous and
    /// short (no I/O, no awaiting).
    pub fn edit<T>(
        &self,
        f: impl FnOnce(&mut RegistryEdit<'_>) -> Result<T, McpMemberError>,
    ) -> Result<T, McpMemberError> {
        let _guard = self
            .inner
            .write
            .lock()
            .expect("MCP registry write lock poisoned");
        let current = self.view();
        let catalog = self.inner.catalog.get().map(|c| &**c);
        let mut edit = RegistryEdit {
            draft: Draft {
                catalog,
                enabled: Vec::new(),
                private: current.private.clone(),
            },
        };
        let out = f(&mut edit)?;
        if edit.draft.private.same_as(&current.private) {
            return Ok(out);
        }
        let filter = catalog.is_some_and(Catalog::filter);
        let next = SessionView::standalone(LABEL, filter, edit.draft.private).map_err(|key| {
            McpMemberError::Duplicate {
                kind: "member",
                key,
            }
        })?;
        let changed = current.diff(&next);
        *self
            .inner
            .view
            .write()
            .expect("MCP registry view lock poisoned") = Arc::new(next);
        drop(_guard);
        if let Some(sessions) = self.inner.sessions.get() {
            sessions.notify(changed);
        }
        Ok(out)
    }

    // ── One-shot edits ───────────────────────────────────────────────────

    /// Add a tool every session serves. Fails on a catalog or registry
    /// namesake ([`McpMemberError::Duplicate`]).
    pub fn add_tool(&self, tool: impl Into<ToolRoute>) -> Result<(), McpMemberError> {
        let tool = tool.into();
        self.edit(|edit| edit.add_tool(tool))
    }

    /// Add or replace (by name) a tool every session serves. `Ok(true)`
    /// when a previous registry tool was replaced.
    pub fn upsert_tool(&self, tool: impl Into<ToolRoute>) -> Result<bool, McpMemberError> {
        let tool = tool.into();
        self.edit(|edit| edit.upsert_tool(tool))
    }

    /// Remove the registry tool named `name`; `false` when there is none.
    pub fn remove_tool(&self, name: &str) -> bool {
        self.edit(|edit| Ok(edit.remove_tool(name)))
            .expect("removing a member cannot fail")
    }

    /// Add a resource (fixed URI or template) every session serves.
    pub fn add_resource(&self, resource: impl Into<ResourceRoute>) -> Result<(), McpMemberError> {
        let resource = resource.into();
        self.edit(|edit| edit.add_resource(resource))
    }

    /// Add or replace (by URI) a resource every session serves.
    pub fn upsert_resource(
        &self,
        resource: impl Into<ResourceRoute>,
    ) -> Result<bool, McpMemberError> {
        let resource = resource.into();
        self.edit(|edit| edit.upsert_resource(resource))
    }

    /// Remove the registry resource with this exact `uri` (a template's
    /// URI text included); `false` when there is none.
    pub fn remove_resource(&self, uri: &str) -> bool {
        self.edit(|edit| Ok(edit.remove_resource(uri)))
            .expect("removing a member cannot fail")
    }

    /// Add a prompt every session serves.
    pub fn add_prompt(&self, prompt: impl Into<PromptRoute>) -> Result<(), McpMemberError> {
        let prompt = prompt.into();
        self.edit(|edit| edit.add_prompt(prompt))
    }

    /// Add or replace (by name) a prompt every session serves.
    pub fn upsert_prompt(&self, prompt: impl Into<PromptRoute>) -> Result<bool, McpMemberError> {
        let prompt = prompt.into();
        self.edit(|edit| edit.upsert_prompt(prompt))
    }

    /// Remove the registry prompt named `name`; `false` when there is none.
    pub fn remove_prompt(&self, name: &str) -> bool {
        self.edit(|edit| Ok(edit.remove_prompt(name)))
            .expect("removing a member cannot fail")
    }

    // ── Reads ────────────────────────────────────────────────────────────

    /// Whether the registry holds a tool named `name`.
    pub fn has_tool(&self, name: &str) -> bool {
        self.view().tools.route(name).is_some()
    }

    /// Whether the registry holds a resource with exactly this `uri`.
    pub fn has_resource(&self, uri: &str) -> bool {
        let view = self.view();
        view.resources.route(uri).is_some() || view.resource_templates.has_raw(uri)
    }

    /// Whether the registry holds a prompt named `name`.
    pub fn has_prompt(&self, name: &str) -> bool {
        self.view().prompts.route(name).is_some()
    }

    /// The registry's tool names, in insertion order.
    pub fn tool_names(&self) -> Vec<String> {
        self.view()
            .private
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    /// Whether the registry holds no member at all.
    pub fn is_empty(&self) -> bool {
        let view = self.view();
        view.private.tools.is_empty()
            && view.private.resources.is_empty()
            && view.private.prompts.is_empty()
    }
}

/// One atomic batch of registry changes — see [`McpRegistry::edit`].
///
/// Every `add_*` fails like the one-shot method would; `remove_*` reports
/// whether a member was removed; `upsert_*` replaces a registry member of
/// the same key (a catalog namesake still fails).
pub struct RegistryEdit<'a> {
    draft: Draft<'a>,
}

impl RegistryEdit<'_> {
    /// Add a tool (fails on a catalog or registry namesake).
    pub fn add_tool(&mut self, tool: impl Into<ToolRoute>) -> Result<(), McpMemberError> {
        self.draft.add_tool(tool.into())
    }

    /// Add or replace the tool of the same name; `true` when replaced.
    pub fn upsert_tool(&mut self, tool: impl Into<ToolRoute>) -> Result<bool, McpMemberError> {
        let tool = tool.into();
        let replaced = self.remove_tool(&tool.name);
        self.draft.add_tool(tool)?;
        Ok(replaced)
    }

    /// Remove the tool named `name`; `false` when there is none.
    pub fn remove_tool(&mut self, name: &str) -> bool {
        remove_by(&mut self.draft.private.tools, |tool| tool.name == name)
    }

    /// Add a resource (fixed URI or template).
    pub fn add_resource(
        &mut self,
        resource: impl Into<ResourceRoute>,
    ) -> Result<(), McpMemberError> {
        self.draft.add_resource(resource.into())
    }

    /// Add or replace the resource of the same URI; `true` when replaced.
    pub fn upsert_resource(
        &mut self,
        resource: impl Into<ResourceRoute>,
    ) -> Result<bool, McpMemberError> {
        let resource = resource.into();
        let replaced = self.remove_resource(&resource.uri);
        self.draft.add_resource(resource)?;
        Ok(replaced)
    }

    /// Remove the resource with exactly this `uri`; `false` when there is
    /// none.
    pub fn remove_resource(&mut self, uri: &str) -> bool {
        remove_by(&mut self.draft.private.resources, |resource| {
            resource.uri == uri
        })
    }

    /// Add a prompt.
    pub fn add_prompt(&mut self, prompt: impl Into<PromptRoute>) -> Result<(), McpMemberError> {
        self.draft.add_prompt(prompt.into())
    }

    /// Add or replace the prompt of the same name; `true` when replaced.
    pub fn upsert_prompt(
        &mut self,
        prompt: impl Into<PromptRoute>,
    ) -> Result<bool, McpMemberError> {
        let prompt = prompt.into();
        let replaced = self.remove_prompt(&prompt.name);
        self.draft.add_prompt(prompt)?;
        Ok(replaced)
    }

    /// Remove the prompt named `name`; `false` when there is none.
    pub fn remove_prompt(&mut self, name: &str) -> bool {
        remove_by(&mut self.draft.private.prompts, |prompt| {
            prompt.name == name
        })
    }
}
