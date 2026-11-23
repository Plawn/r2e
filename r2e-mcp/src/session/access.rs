//! What one request sees of the members: the session's view merged with the
//! global registry's, narrowed by group state and by the application's
//! [`McpViewFilter`].
//!
//! Three things decide whether a member exists for a request, in this
//! order:
//!
//! 1. its **group** — a catalog or session-private member of a group that
//!    is off is not in the session's view at all (the view is rebuilt on a
//!    group change); a registry member's group is checked here, against the
//!    asking session, so a registry change never rebuilds a session;
//! 2. the **view filter**, when one is installed
//!    ([`McpServer::view_filter`](crate::McpServer::view_filter)) — asked
//!    about every member, on every list and every lookup;
//! 3. on lists only, the filter's [`McpViewFilter::listed`] (a member that
//!    exists may be left out of the lists) and the caller's scopes and roles
//!    under `mcp.auth.filter-members` (a call enforces them in the member).
//!
//! A member the first two hide answers exactly like an unknown one. The
//! third only ever narrows a list: a list never advertises a member a
//! lookup would not find.

use std::collections::BTreeMap;
use std::sync::Arc;

use r2e_core::http::Parts;
use rmcp::model::{MetaObject, Prompt, Resource, ResourceTemplate, Tool};

use super::id::SessionId;
use super::state::SessionState;
use crate::auth::tools::ToolRequirements;
use crate::auth::McpPrincipal;
use crate::catalog::{Admit, SessionView};
use crate::identity::caller_in;
use crate::route::{identity_in, McpGroup, PromptRoute, ResourceRoute, ToolRoute};

/// A member as a [`McpViewFilter`] sees it: the route the `#[routes]` macro
/// or a `Dynamic*` builder produced.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum McpMember<'a> {
    /// A tool.
    Tool(&'a ToolRoute),
    /// A resource — fixed URI or template
    /// ([`ResourceRoute::is_template`]).
    Resource(&'a ResourceRoute),
    /// A prompt.
    Prompt(&'a PromptRoute),
}

impl<'a> McpMember<'a> {
    /// The member's key: a tool or prompt name, a resource URI (the
    /// template text for a template).
    pub fn name(&self) -> &'a str {
        match *self {
            McpMember::Tool(tool) => &tool.name,
            McpMember::Resource(resource) => &resource.uri,
            McpMember::Prompt(prompt) => &prompt.name,
        }
    }

    /// The member's group name, if it is in one.
    pub fn group(&self) -> Option<&'a str> {
        let group = match *self {
            McpMember::Tool(tool) => &tool.group,
            McpMember::Resource(resource) => &resource.group,
            McpMember::Prompt(prompt) => &prompt.group,
        };
        group.as_ref().map(|group| group.name.as_ref())
    }

    /// The `_meta` object the member advertises, if any — where an
    /// application tags its members (a tool class, an owning plugin) for
    /// its filter to read.
    pub fn meta(&self) -> Option<&'a MetaObject> {
        match *self {
            McpMember::Tool(tool) => tool.meta.as_ref(),
            McpMember::Resource(resource) => resource.meta.as_ref(),
            McpMember::Prompt(prompt) => prompt.meta.as_ref(),
        }
    }

    /// The scopes and roles the member declares.
    pub fn requirements(&self) -> &'a ToolRequirements {
        match *self {
            McpMember::Tool(tool) => &tool.requirements,
            McpMember::Resource(resource) => &resource.requirements,
            McpMember::Prompt(prompt) => &prompt.requirements,
        }
    }

    /// The tool, when the member is one.
    pub fn as_tool(&self) -> Option<&'a ToolRoute> {
        match *self {
            McpMember::Tool(tool) => Some(tool),
            _ => None,
        }
    }

    /// Whether the member is a tool.
    pub fn is_tool(&self) -> bool {
        matches!(self, McpMember::Tool(_))
    }

    /// Whether the member is a resource (fixed URI or template).
    pub fn is_resource(&self) -> bool {
        matches!(self, McpMember::Resource(_))
    }

    /// Whether the member is a prompt.
    pub fn is_prompt(&self) -> bool {
        matches!(self, McpMember::Prompt(_))
    }
}

/// What a [`McpViewFilter`] knows about the request it filters for: the
/// caller, the request extensions and the session.
pub struct McpView<'a> {
    parts: Option<&'a Parts>,
    state: &'a SessionState,
    view: &'a SessionView,
    registry: &'a SessionView,
}

impl<'a> McpView<'a> {
    /// The transport session id — `None` for a sessionless request
    /// (`mcp.stateless`, a 2026-07-28 client) and while the `initialize`
    /// handshake runs.
    pub fn session_id(&self) -> Option<&'a SessionId> {
        self.state.id()
    }

    /// Whether the request belongs to a real MCP session.
    pub fn is_persistent(&self) -> bool {
        self.state.is_persistent()
    }

    /// Whether the group `name` is on for this session: a catalog group or
    /// one a dynamic member declares. `false` for an unknown name.
    pub fn is_group_enabled(&self, name: &str) -> bool {
        self.state.group_enabled(self.view, self.registry, name)
    }

    /// The authenticated MCP caller, when `mcp.auth` is on.
    pub fn principal(&self) -> Option<&'a McpPrincipal> {
        self.parts?.extensions.get::<McpPrincipal>()
    }

    /// The caller's subject (`sub` claim), when authenticated — by the
    /// `mcp.auth` principal or a service's struct-level identity.
    pub fn subject(&self) -> Option<&'a str> {
        caller_in(self.parts).map(|caller| caller.sub)
    }

    /// Whether the authenticated caller holds `role`.
    pub fn has_role(&self, role: &str) -> bool {
        caller_in(self.parts).is_some_and(|caller| caller.has_role(role))
    }

    /// Whether the caller's token carries `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.principal().is_some_and(|p| p.has_scope(scope))
    }

    /// Resolve a request-scoped identity — same semantics as
    /// [`ToolCall::identity`](crate::ToolCall::identity).
    pub fn identity<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        identity_in(self.parts)
    }

    /// A request extension, by reference: inserted by an HTTP layer, or
    /// attached to a loopback session at connect.
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<&'a T> {
        self.parts?.extensions.get::<T>()
    }

    /// A request header value (UTF-8 only).
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.parts?.headers.get(name)?.to_str().ok()
    }

    /// The HTTP request head the message arrived with.
    pub fn parts(&self) -> Option<&'a Parts> {
        self.parts
    }
}

/// Decide, per request, which members exist for the caller.
///
/// Implemented by a bean and installed with
/// [`McpServer::view_filter`](crate::McpServer::view_filter). It is asked
/// about every member the session would otherwise serve — catalog,
/// session-private and registry alike — on every list **and** every lookup
/// (`tools/call`, `resources/read`, `resources/subscribe`, `prompts/get`,
/// `completion/complete`): a member it hides answers exactly like an
/// unknown one, so a list never shows a member a call would not find.
///
/// It runs synchronously on the request path, once per member on a list:
/// decide from what [`McpView`] and [`McpMember`] carry plus state the bean
/// already holds in memory — no I/O. When that state changes (a tool class
/// unlocked for a caller), tell the clients to re-list with
/// [`McpSession::notify_list_changed`](crate::McpSession::notify_list_changed)
/// or [`McpSessions::notify_list_changed`](crate::McpSessions::notify_list_changed).
///
/// [`listed`](Self::listed) separates the two questions when they differ: a
/// member that exists but is left out of the lists (a long tail of tools a
/// client may still call by name, a deprecated member kept for old clients).
///
/// ```ignore
/// #[derive(Clone)]
/// struct ByPlan;
///
/// impl McpViewFilter for ByPlan {
///     fn visible(&self, member: McpMember<'_>, view: &McpView<'_>) -> bool {
///         match member.group() {
///             Some("billing") => view.has_role("finance"),
///             _ => true,
///         }
///     }
/// }
/// ```
pub trait McpViewFilter: Send + Sync + 'static {
    /// Whether `member` exists for the request `view` describes: asked on
    /// every list and every lookup by name.
    fn visible(&self, member: McpMember<'_>, view: &McpView<'_>) -> bool;

    /// Whether a member that exists is advertised by the list methods
    /// (`tools/list`, `resources/list`, `resources/templates/list`,
    /// `prompts/list`). Asked on lists only, and only about members
    /// [`visible`](Self::visible) accepted — so it can take a member out of
    /// a list, never put a hidden one in.
    ///
    /// Defaults to `true`: what exists is listed. A member it leaves out
    /// still answers every lookup by name (`tools/call`, `resources/read`,
    /// `resources/subscribe`, `prompts/get`, `completion/complete`) — this
    /// is curation, not access control; a member the caller must not reach
    /// is one `visible` hides.
    fn listed(&self, _member: McpMember<'_>, _view: &McpView<'_>) -> bool {
        true
    }
}

/// Type-erased [`McpViewFilter`].
pub(crate) type ViewFilterFn = Arc<dyn McpViewFilter>;

pub(crate) fn erase_view_filter<T: McpViewFilter>(filter: T) -> ViewFilterFn {
    Arc::new(filter)
}

/// A route a request-time check can be asked about.
pub(crate) trait Gated {
    fn group(&self) -> Option<&McpGroup>;
    fn member(&self) -> McpMember<'_>;
}

impl Gated for ToolRoute {
    fn group(&self) -> Option<&McpGroup> {
        self.group.as_ref()
    }

    fn member(&self) -> McpMember<'_> {
        McpMember::Tool(self)
    }
}

impl Gated for ResourceRoute {
    fn group(&self) -> Option<&McpGroup> {
        self.group.as_ref()
    }

    fn member(&self) -> McpMember<'_> {
        McpMember::Resource(self)
    }
}

impl Gated for PromptRoute {
    fn group(&self) -> Option<&McpGroup> {
        self.group.as_ref()
    }

    fn member(&self) -> McpMember<'_> {
        McpMember::Prompt(self)
    }
}

/// One request's window on the members: a consistent snapshot of the
/// session's view and the registry's, the caller, and the view filter.
///
/// Resolution order is the session's view, then the registry: a
/// session-private member shadows a registry namesake — unless the filter
/// hides it, in which case the registry's is served (a hidden member is
/// absent, it does not shadow). The catalog never clashes with either
/// (checked when they are added).
pub(crate) struct Access<'a> {
    state: &'a SessionState,
    view: Arc<SessionView>,
    registry: Arc<SessionView>,
    parts: Option<&'a Parts>,
    filter: Option<&'a ViewFilterFn>,
}

impl<'a> Access<'a> {
    pub(super) fn new(
        state: &'a SessionState,
        view: Arc<SessionView>,
        registry: Arc<SessionView>,
        parts: Option<&'a Parts>,
        filter: Option<&'a ViewFilterFn>,
    ) -> Self {
        Access {
            state,
            view,
            registry,
            parts,
            filter,
        }
    }

    fn request(&self) -> McpView<'_> {
        McpView {
            parts: self.parts,
            state: self.state,
            view: &self.view,
            registry: &self.registry,
        }
    }

    /// Whether the filter lets `member` exist for this request.
    fn filtered(&self, member: McpMember<'_>) -> bool {
        self.filter
            .is_none_or(|filter| filter.visible(member, &self.request()))
    }

    /// Whether the filter lets `member` exist **and** be listed.
    fn advertised(&self, member: McpMember<'_>) -> bool {
        self.filter.is_none_or(|filter| {
            let view = self.request();
            filter.visible(member, &view) && filter.listed(member, &view)
        })
    }

    fn group_on<R: Gated>(&self, route: &R) -> bool {
        route
            .group()
            .is_none_or(|group| self.view.group_on(self.state.catalog().keys(), group))
    }

    /// Whether a member of the session's own view exists for this request.
    fn own<R: Gated>(&self, route: &R) -> bool {
        self.filtered(route.member())
    }

    /// Whether a registry member exists for this request: its group is on
    /// for the session, and the filter lets it through.
    fn global<R: Gated>(&self, route: &R) -> bool {
        self.group_on(route) && self.filtered(route.member())
    }

    /// Whether a member of the session's own view goes in a list.
    fn own_listed<R: Gated>(&self, route: &R) -> bool {
        self.advertised(route.member())
    }

    /// Whether a registry member goes in a list.
    fn global_listed<R: Gated>(&self, route: &R) -> bool {
        self.group_on(route) && self.advertised(route.member())
    }

    fn own_gated(&self) -> bool {
        self.filter.is_some()
    }

    fn global_gated(&self) -> bool {
        self.filter.is_some() || self.registry.grouped
    }

    // ── Lookups ──────────────────────────────────────────────────────────

    /// The tool `name` resolves to for this request.
    pub(crate) fn tool(&self, name: &str) -> Option<Arc<ToolRoute>> {
        self.view
            .tools
            .route(name)
            .filter(|route| self.own(&***route))
            .or_else(|| {
                let route = self.registry.tools.route(name)?;
                self.global(&**route).then_some(route)
            })
            .map(Arc::clone)
    }

    /// The resource `uri` resolves to (fixed URI, then template; session,
    /// then registry), with the template variables it captured.
    pub(crate) fn resource(
        &self,
        uri: &str,
    ) -> Option<(Arc<ResourceRoute>, BTreeMap<String, String>)> {
        self.view
            .resource_route(uri)
            .filter(|(route, _)| self.own(&***route))
            .or_else(|| {
                self.registry
                    .resource_route(uri)
                    .filter(|(route, _)| self.global(&***route))
            })
            .map(|(route, variables)| (Arc::clone(route), variables))
    }

    /// The prompt `name` resolves to for this request.
    pub(crate) fn prompt(&self, name: &str) -> Option<Arc<PromptRoute>> {
        self.view
            .prompts
            .route(name)
            .filter(|route| self.own(&***route))
            .or_else(|| {
                let route = self.registry.prompts.route(name)?;
                self.global(&**route).then_some(route)
            })
            .map(Arc::clone)
    }

    /// The resource a `completion/complete` `ref/resource` names.
    pub(crate) fn completion_resource(&self, uri: &str) -> Option<Arc<ResourceRoute>> {
        self.view
            .completion_resource(uri)
            .filter(|route| self.own(&***route))
            .or_else(|| {
                let route = self.registry.completion_resource(uri)?;
                self.global(&**route).then_some(route)
            })
            .map(Arc::clone)
    }

    // ── List payloads: the session's list followed by the registry's,
    // built into one allocation. With an empty registry and no filter (the
    // common deployment) this is exactly the prebuilt list's clone.
    //
    // A member is admitted when it exists and the filter lists it; a
    // registry namesake is shadowed by a session-private member that
    // *exists*, listed or not — a call resolves to the private one, so the
    // list must not describe the other.

    pub(crate) fn tool_list(&self) -> Vec<Tool> {
        let caller = caller_in(self.parts);
        let caller = caller.as_ref();
        let own = |route: &ToolRoute| self.own_listed(route);
        let own: Admit<'_, ToolRoute> = self.own_gated().then_some(&own);
        let (view, registry) = (&self.view.tools, &self.registry.tools);
        if registry.is_empty() {
            return view.visible_list(caller, own);
        }
        let global = |route: &ToolRoute| self.global_listed(route);
        let global: Admit<'_, ToolRoute> = self.global_gated().then_some(&global);
        let mut out = Vec::with_capacity(view.len() + registry.len());
        view.extend_visible(caller, own, &mut out, |_| false);
        if self.view.private.tools.is_empty() {
            registry.extend_visible(caller, global, &mut out, |_| false);
        } else {
            registry.extend_visible(caller, global, &mut out, |name| {
                view.route(name).is_some_and(|route| self.own(&**route))
            });
        }
        out
    }

    pub(crate) fn resource_list(&self) -> Vec<Resource> {
        let caller = caller_in(self.parts);
        let caller = caller.as_ref();
        let own = |route: &ResourceRoute| self.own_listed(route);
        let own: Admit<'_, ResourceRoute> = self.own_gated().then_some(&own);
        let (view, registry) = (&self.view.resources, &self.registry.resources);
        if registry.is_empty() {
            return view.visible_list(caller, own);
        }
        let global = |route: &ResourceRoute| self.global_listed(route);
        let global: Admit<'_, ResourceRoute> = self.global_gated().then_some(&global);
        let mut out = Vec::with_capacity(view.len() + registry.len());
        view.extend_visible(caller, own, &mut out, |_| false);
        if self.view.private.resources.is_empty() {
            registry.extend_visible(caller, global, &mut out, |_| false);
        } else {
            registry.extend_visible(caller, global, &mut out, |uri| {
                view.route(uri).is_some_and(|route| self.own(&**route))
            });
        }
        out
    }

    pub(crate) fn resource_template_list(&self) -> Vec<ResourceTemplate> {
        let caller = caller_in(self.parts);
        let caller = caller.as_ref();
        let own = |route: &ResourceRoute| self.own_listed(route);
        let own: Admit<'_, ResourceRoute> = self.own_gated().then_some(&own);
        let (view, registry) = (
            &self.view.resource_templates,
            &self.registry.resource_templates,
        );
        if registry.is_empty() {
            return view.visible_list(caller, own);
        }
        let global = |route: &ResourceRoute| self.global_listed(route);
        let global: Admit<'_, ResourceRoute> = self.global_gated().then_some(&global);
        let mut out = Vec::with_capacity(view.len() + registry.len());
        view.extend_visible(caller, own, &mut out, |_| false);
        if self.view.private.resources.is_empty() {
            registry.extend_visible(caller, global, &mut out, |_| false);
        } else {
            registry.extend_visible(caller, global, &mut out, |raw| {
                view.raw_route(raw).is_some_and(|route| self.own(&**route))
            });
        }
        out
    }

    pub(crate) fn prompt_list(&self) -> Vec<Prompt> {
        let caller = caller_in(self.parts);
        let caller = caller.as_ref();
        let own = |route: &PromptRoute| self.own_listed(route);
        let own: Admit<'_, PromptRoute> = self.own_gated().then_some(&own);
        let (view, registry) = (&self.view.prompts, &self.registry.prompts);
        if registry.is_empty() {
            return view.visible_list(caller, own);
        }
        let global = |route: &PromptRoute| self.global_listed(route);
        let global: Admit<'_, PromptRoute> = self.global_gated().then_some(&global);
        let mut out = Vec::with_capacity(view.len() + registry.len());
        view.extend_visible(caller, own, &mut out, |_| false);
        if self.view.private.prompts.is_empty() {
            registry.extend_visible(caller, global, &mut out, |_| false);
        } else {
            registry.extend_visible(caller, global, &mut out, |name| {
                view.route(name).is_some_and(|route| self.own(&**route))
            });
        }
        out
    }
}
