//! The boot-time member catalog and the per-session views derived from it.
//!
//! [`Catalog`] is immutable: every `#[mcp_routes]` member of every registered
//! service, with its group. A [`SessionView`] is what one session serves —
//! the catalog members of its enabled groups plus its session-private
//! members — with the wire lists precomputed. Sessions that never reshape
//! their list share [`Catalog::default_view`], so the common case costs one
//! `Arc` clone per session.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use r2e_core::http::Parts;
use rmcp::model::{
    Implementation, Prompt, PromptsCapability, Resource, ResourceTemplate, ResourcesCapability,
    ServerCapabilities, ServerInfo, Tool, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};

use crate::auth::tools::{requirements_visible, ToolRequirements};
use crate::auth::McpPrincipal;
use crate::registry::RegisteredMcpService;
use crate::route::{McpGroup, PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

/// Server identity/behavior settings resolved by the plugin (builder
/// overrides > `mcp.*` config > defaults).
pub(crate) struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
}

/// Boot-time switches that shape the catalog.
pub(crate) struct CatalogOptions {
    /// `mcp.auth.filter-members` (and auth on).
    pub filter_members: bool,
    /// `mcp.auth` configured.
    pub auth_enabled: bool,
    /// MCP sessions exist (`mcp.stateless = false`).
    pub stateful: bool,
    /// A session-init hook is installed.
    pub session_init: bool,
}

/// The auth layer's principal, carried in the HTTP request parts that the
/// transport copies into the request extensions.
pub(crate) fn principal_in(parts: Option<&Parts>) -> Option<&McpPrincipal> {
    parts.and_then(|parts| parts.extensions.get::<McpPrincipal>())
}

fn principal_of(context: &RequestContext<RoleServer>) -> Option<&McpPrincipal> {
    principal_in(context.extensions.get::<Parts>())
}

/// One member family's dispatch table: routes by key, the precomputed list
/// payload, and the parallel requirements for the visibility filter.
pub(crate) struct Family<R, W> {
    routes: HashMap<String, FamilyRoute<R>>,
    list: Vec<W>,
    reqs: Vec<ToolRequirements>,
    /// Per-caller list filtering (`mcp.auth.filter-members`). Off — or no
    /// member has requirements — means the precomputed list goes out as-is.
    filter: bool,
}

/// Dispatch data for one member plus its slot in the precomputed wire list.
///
/// rmcp asks for a tool descriptor to validate `Mcp-Param-*` headers.
/// Keeping the list index here lets that path clone the descriptor built at
/// boot instead of rebuilding its strings, annotations and schema pointers.
struct FamilyRoute<R> {
    route: Arc<R>,
    wire_index: usize,
}

/// A family member ready to be folded: owning service (for boot
/// diagnostics), dispatch key, requirements, wire descriptor, route.
type FamilyMember<R, W> = (&'static str, String, ToolRequirements, W, Arc<R>);

impl<R, W: Clone> Family<R, W> {
    /// Fold members into a dispatch table; `Err` names the first duplicate.
    fn build(kind: &str, filter: bool, members: Vec<FamilyMember<R, W>>) -> Result<Self, String> {
        let capacity = members.len();
        let mut routes = HashMap::with_capacity(capacity);
        let mut owners: HashMap<String, &'static str> = HashMap::with_capacity(capacity);
        let mut list = Vec::with_capacity(capacity);
        let mut reqs = Vec::with_capacity(capacity);
        for (service, key, req, wire, route) in members {
            if let Some(previous) = owners.get(key.as_str()) {
                return Err(format!(
                    "duplicate MCP {kind} `{key}`: registered by both `{previous}` and \
                     `{service}` — {kind}s are global across services"
                ));
            }
            owners.insert(key.clone(), service);
            let wire_index = list.len();
            list.push(wire);
            reqs.push(req);
            routes.insert(key, FamilyRoute { route, wire_index });
        }
        let filter = filter && reqs.iter().any(|r| !r.is_empty());
        Ok(Family {
            routes,
            list,
            reqs,
            filter,
        })
    }

    /// The list payload for this caller: precomputed when unfiltered,
    /// otherwise the members whose requirements the caller satisfies.
    pub(crate) fn visible_list(&self, context: &RequestContext<RoleServer>) -> Vec<W> {
        if !self.filter {
            return self.list.clone();
        }
        let principal = principal_of(context);
        self.list
            .iter()
            .zip(self.reqs.iter())
            .filter(|(_, req)| requirements_visible(principal, req))
            .map(|(wire, _)| wire.clone())
            .collect()
    }

    pub(crate) fn route(&self, key: &str) -> Option<&Arc<R>> {
        self.routes.get(key).map(|entry| &entry.route)
    }

    fn wire(&self, key: &str) -> Option<W> {
        let entry = self.routes.get(key)?;
        self.list.get(entry.wire_index).cloned()
    }

    /// Whether `other` serves exactly the same members (same keys, same
    /// route objects) — the "did this family's list change" test.
    fn same_members(&self, other: &Self) -> bool {
        self.routes.len() == other.routes.len()
            && self.routes.iter().all(|(key, entry)| {
                other
                    .routes
                    .get(key)
                    .is_some_and(|o| Arc::ptr_eq(&o.route, &entry.route))
            })
    }
}

struct TemplateFamilyRoute {
    matcher: UriTemplate,
    route: Arc<ResourceRoute>,
}

pub(crate) struct TemplateFamily {
    routes: Vec<TemplateFamilyRoute>,
    list: Vec<ResourceTemplate>,
    reqs: Vec<ToolRequirements>,
    filter: bool,
}

type TemplateMember = (
    &'static str,
    ToolRequirements,
    ResourceTemplate,
    Arc<ResourceRoute>,
);

impl TemplateFamily {
    fn build(filter: bool, members: Vec<TemplateMember>) -> Result<Self, String> {
        let mut owners = HashMap::with_capacity(members.len());
        let mut routes = Vec::with_capacity(members.len());
        let mut list = Vec::with_capacity(members.len());
        let mut reqs = Vec::with_capacity(members.len());
        for (service, req, wire, route) in members {
            let matcher = UriTemplate::parse(route.uri.as_ref()).map_err(|error| {
                format!(
                    "invalid MCP resource URI template `{}` in `{service}`: {error}",
                    route.uri
                )
            })?;
            // Keyed by shape, not text: `{id}` and `{uid}` match the same
            // URIs and the first registered would silently shadow the other.
            if let Some((previous_service, previous_raw)) =
                owners.insert(matcher.shape(), (service, matcher.raw().to_string()))
            {
                return Err(format!(
                    "duplicate MCP resource URI template `{}` (`{previous_raw}` in \
                     `{previous_service}` matches the same URIs): registered by both \
                     `{previous_service}` and `{service}` — resource templates are global \
                     across services",
                    matcher.raw()
                ));
            }
            routes.push(TemplateFamilyRoute { matcher, route });
            list.push(wire);
            reqs.push(req);
        }
        let filter = filter && reqs.iter().any(|r| !r.is_empty());
        Ok(Self {
            routes,
            list,
            reqs,
            filter,
        })
    }

    pub(crate) fn visible_list(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Vec<ResourceTemplate> {
        if !self.filter {
            return self.list.clone();
        }
        let principal = principal_of(context);
        self.list
            .iter()
            .zip(self.reqs.iter())
            .filter(|(_, req)| requirements_visible(principal, req))
            .map(|(wire, _)| wire.clone())
            .collect()
    }

    fn route(&self, uri: &str) -> Option<(&Arc<ResourceRoute>, BTreeMap<String, String>)> {
        self.routes.iter().find_map(|entry| {
            entry
                .matcher
                .captures(uri)
                .map(|variables| (&entry.route, variables))
        })
    }

    /// The template a `completion/complete` `ref/resource` names: the exact
    /// template text, else any template matching the same URIs (the client
    /// may spell the variables differently).
    fn template(&self, raw: &str) -> Option<&Arc<ResourceRoute>> {
        if let Some(entry) = self.routes.iter().find(|e| e.matcher.raw() == raw) {
            return Some(&entry.route);
        }
        let shape = UriTemplate::parse(raw).ok()?.shape();
        self.routes
            .iter()
            .find(|e| e.matcher.shape() == shape)
            .map(|e| &e.route)
    }

    fn same_members(&self, other: &Self) -> bool {
        self.routes.len() == other.routes.len()
            && self
                .routes
                .iter()
                .zip(other.routes.iter())
                .all(|(a, b)| Arc::ptr_eq(&a.route, &b.route))
    }
}

/// A catalog member: the route plus what a view rebuild needs.
struct Member<R, W> {
    service: &'static str,
    key: String,
    requirements: ToolRequirements,
    wire: W,
    route: Arc<R>,
    group: Option<usize>,
}

struct GroupDef {
    name: Cow<'static, str>,
    opt_in: bool,
    /// The service that first declared the group (boot diagnostics).
    owner: &'static str,
}

/// Session-private members, in insertion order.
#[derive(Clone, Default)]
pub(crate) struct PrivateMembers {
    pub tools: Vec<Arc<ToolRoute>>,
    pub resources: Vec<Arc<ResourceRoute>>,
    pub prompts: Vec<Arc<PromptRoute>>,
}

impl PrivateMembers {
    /// Whether both hold the same route objects, in the same order.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        fn same<T>(a: &[Arc<T>], b: &[Arc<T>]) -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y))
        }
        same(&self.tools, &other.tools)
            && same(&self.resources, &other.resources)
            && same(&self.prompts, &other.prompts)
    }
}

/// Which families a view change touched — one `list_changed` notification
/// per flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changed {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl Changed {
    pub(crate) const ALL: Changed = Changed {
        tools: true,
        resources: true,
        prompts: true,
    };

    pub(crate) fn any(self) -> bool {
        self.tools || self.resources || self.prompts
    }
}

/// What one session serves.
pub(crate) struct SessionView {
    /// Per catalog group (same index as the catalog's group table).
    pub(crate) enabled: Box<[bool]>,
    pub(crate) private: PrivateMembers,
    pub(crate) tools: Family<ToolRoute, Tool>,
    pub(crate) resources: Family<ResourceRoute, Resource>,
    pub(crate) resource_templates: TemplateFamily,
    pub(crate) prompts: Family<PromptRoute, Prompt>,
}

impl SessionView {
    pub(crate) fn resource_route(
        &self,
        uri: &str,
    ) -> Option<(&Arc<ResourceRoute>, BTreeMap<String, String>)> {
        self.resources
            .route(uri)
            .map(|route| (route, BTreeMap::new()))
            .or_else(|| self.resource_templates.route(uri))
    }

    /// The resource a `completion/complete` `ref/resource` names: a
    /// template (see [`TemplateFamily::template`]) or a fixed URI.
    pub(crate) fn completion_resource(&self, uri: &str) -> Option<&Arc<ResourceRoute>> {
        self.resource_templates
            .template(uri)
            .or_else(|| self.resources.route(uri))
    }

    pub(crate) fn has_resource(&self, uri: &str) -> bool {
        self.resource_route(uri).is_some()
    }

    /// The families whose served members differ between `self` and `next`.
    pub(crate) fn diff(&self, next: &SessionView) -> Changed {
        Changed {
            tools: !self.tools.same_members(&next.tools),
            resources: !self.resources.same_members(&next.resources)
                || !self
                    .resource_templates
                    .same_members(&next.resource_templates),
            prompts: !self.prompts.same_members(&next.prompts),
        }
    }
}

/// Every catalog member, per family.
struct Members {
    tools: Vec<Member<ToolRoute, Tool>>,
    resources: Vec<Member<ResourceRoute, Resource>>,
    templates: Vec<Member<ResourceRoute, ResourceTemplate>>,
    prompts: Vec<Member<PromptRoute, Prompt>>,
}

/// The immutable member catalog built once when the router is assembled.
pub(crate) struct Catalog {
    pub(crate) info: ServerInfo,
    filter: bool,
    /// MCP sessions exist — a mutation can outlive its request.
    pub(crate) stateful: bool,
    /// `mcp.auth` is on: session-private members may declare scopes.
    pub(crate) auth_enabled: bool,
    groups: Vec<GroupDef>,
    group_index: HashMap<Cow<'static, str>, usize>,
    members: Members,
    /// Every catalog key, hidden groups included: a session-private member
    /// may not reuse one (it would shadow a member another session sees).
    tool_keys: HashSet<String>,
    resource_keys: HashSet<String>,
    template_shapes: HashSet<String>,
    prompt_keys: HashSet<String>,
    /// Tool descriptors of every catalog tool, for [`Catalog::tool_wire`].
    all_tools: Family<ToolRoute, Tool>,
    pub(crate) default_view: Arc<SessionView>,
}

impl Catalog {
    /// Fold the drained services into the catalog.
    ///
    /// # Panics
    ///
    /// Panics when two services register the same tool name, resource URI or
    /// prompt name (hidden groups included — the MCP equivalent of an HTTP
    /// route conflict, surfaced at boot with both service names), when
    /// members disagree on a group's `opt_in`, or when a member takes an
    /// `McpSession` under `mcp.stateless`.
    pub(crate) fn build(
        services: Vec<RegisteredMcpService>,
        identity: ServerIdentity,
        options: CatalogOptions,
    ) -> Self {
        let mut groups: Vec<GroupDef> = Vec::new();
        let mut group_index: HashMap<Cow<'static, str>, usize> = HashMap::new();
        let mut uses_session = false;
        let mut has_completions = false;
        let mut tools = Vec::new();
        let mut resources = Vec::new();
        let mut templates = Vec::new();
        let mut prompts = Vec::new();

        let mut intern = |service: &'static str, group: &Option<McpGroup>| -> Option<usize> {
            let group = group.as_ref()?;
            if let Some(&index) = group_index.get(&group.name) {
                let def: &GroupDef = &groups[index];
                if def.opt_in != group.opt_in {
                    panic!(
                        "MCP group `{}` is declared opt-in by `{}` but not by `{}` \
                         (or vice versa) — every member of a group must agree on `opt_in`",
                        group.name,
                        if def.opt_in { def.owner } else { service },
                        if def.opt_in { service } else { def.owner },
                    );
                }
                return Some(index);
            }
            let index = groups.len();
            groups.push(GroupDef {
                name: group.name.clone(),
                opt_in: group.opt_in,
                owner: service,
            });
            group_index.insert(group.name.clone(), index);
            Some(index)
        };

        for service in services {
            let name = service.name;
            uses_session |= service.routes.uses_session;
            for tool in service.routes.tools {
                require_auth_for_scopes(
                    options.auth_enabled,
                    "tool",
                    &tool.name,
                    &tool.requirements,
                );
                let group = intern(name, &tool.group);
                tools.push(Member {
                    service: name,
                    key: tool.name.to_string(),
                    requirements: tool.requirements,
                    wire: tool.to_rmcp_tool(),
                    route: Arc::new(tool),
                    group,
                });
            }
            for resource in service.routes.resources {
                require_auth_for_scopes(
                    options.auth_enabled,
                    "resource",
                    &resource.uri,
                    &resource.requirements,
                );
                if let Err(reason) = resource_completions_valid(&resource) {
                    panic!("{reason} (in `{name}`)");
                }
                has_completions |= !resource.completions.is_empty();
                let group = intern(name, &resource.group);
                if resource.is_template() {
                    templates.push(Member {
                        service: name,
                        key: resource.uri.to_string(),
                        requirements: resource.requirements,
                        wire: resource.to_rmcp_resource_template(),
                        route: Arc::new(resource),
                        group,
                    });
                } else {
                    resources.push(Member {
                        service: name,
                        key: resource.uri.to_string(),
                        requirements: resource.requirements,
                        wire: resource.to_rmcp_resource(),
                        route: Arc::new(resource),
                        group,
                    });
                }
            }
            for prompt in service.routes.prompts {
                require_auth_for_scopes(
                    options.auth_enabled,
                    "prompt",
                    &prompt.name,
                    &prompt.requirements,
                );
                if let Err(reason) = prompt_completions_valid(&prompt) {
                    panic!("{reason} (in `{name}`)");
                }
                has_completions |= !prompt.completions.is_empty();
                let group = intern(name, &prompt.group);
                prompts.push(Member {
                    service: name,
                    key: prompt.name.to_string(),
                    requirements: prompt.requirements,
                    wire: prompt.to_rmcp_prompt(),
                    route: Arc::new(prompt),
                    group,
                });
            }
        }

        if uses_session && !options.stateful {
            panic!(
                "an MCP member takes an `McpSession` parameter but `mcp.stateless` is \
                 true: without MCP sessions a session change cannot outlive its request. \
                 Serve with sessions (`mcp.stateless: false`) or shape per-caller lists \
                 with `McpServer::session_init` instead"
            );
        }

        // Duplicate detection runs over EVERY member (all groups enabled):
        // a clash between two hidden groups is still a clash for the session
        // that enables both.
        let all_enabled = vec![true; groups.len()].into_boxed_slice();
        let all_tools = Family::build(
            "tool name",
            options.filter_members,
            family_members(&tools, &all_enabled),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        Family::build(
            "resource URI",
            false,
            family_members(&resources, &all_enabled),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let template_shapes =
            TemplateFamily::build(false, template_members(&templates, &all_enabled))
                .unwrap_or_else(|e| panic!("{e}"))
                .routes
                .iter()
                .map(|entry| entry.matcher.shape())
                .collect();
        Family::build("prompt name", false, family_members(&prompts, &all_enabled))
            .unwrap_or_else(|e| panic!("{e}"));

        let dynamic = uses_session || options.session_init || groups.iter().any(|g| g.opt_in);

        let mut info = ServerInfo::default();
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        // Tools are always advertised (the endpoint's primary purpose);
        // resources/prompts only when at least one exists — or when session
        // lists are dynamic, since a session may gain one later. The
        // typestate builder cannot enable conditionally, so set the fields.
        let list_changed = dynamic.then_some(true);
        if dynamic {
            let mut tools_capability = ToolsCapability::default();
            tools_capability.list_changed = list_changed;
            capabilities.tools = Some(tools_capability);
        }
        if dynamic || !resources.is_empty() || !templates.is_empty() {
            let mut resource_capabilities = ResourcesCapability::default();
            resource_capabilities.subscribe = Some(true);
            resource_capabilities.list_changed = list_changed;
            capabilities.resources = Some(resource_capabilities);
        }
        if dynamic || !prompts.is_empty() {
            let mut prompt_capabilities = PromptsCapability::default();
            prompt_capabilities.list_changed = list_changed;
            capabilities.prompts = Some(prompt_capabilities);
        }
        // Same rule for completion: advertised when a provider exists, or
        // when a session may gain one (a dynamic member `with_completion`).
        if dynamic || has_completions {
            capabilities.completions = Some(Default::default());
        }
        info.capabilities = capabilities;
        info.server_info = Implementation::new(identity.name, identity.version);
        info.instructions = identity.instructions;

        let tool_keys = tools.iter().map(|m| m.key.clone()).collect();
        let resource_keys = resources.iter().map(|m| m.key.clone()).collect();
        let prompt_keys = prompts.iter().map(|m| m.key.clone()).collect();
        let members = Members {
            tools,
            resources,
            templates,
            prompts,
        };
        let default_enabled: Box<[bool]> = groups.iter().map(|g| !g.opt_in).collect();
        let default_view = members
            .view(
                options.filter_members,
                default_enabled,
                PrivateMembers::default(),
            )
            .expect("catalog members were validated above");
        Catalog {
            info,
            filter: options.filter_members,
            stateful: options.stateful,
            auth_enabled: options.auth_enabled,
            tool_keys,
            resource_keys,
            template_shapes,
            prompt_keys,
            groups,
            group_index,
            members,
            all_tools,
            default_view: Arc::new(default_view),
        }
    }

    /// Build the view serving the catalog members of the `enabled` groups
    /// plus `private`. `Err` only when a private member collides (callers
    /// check keys first, so this is a backstop).
    pub(crate) fn view(
        &self,
        enabled: Box<[bool]>,
        private: PrivateMembers,
    ) -> Result<SessionView, String> {
        self.members.view(self.filter, enabled, private)
    }

    pub(crate) fn group(&self, name: &str) -> Option<usize> {
        self.group_index.get(name).copied()
    }

    /// The descriptor of any catalog tool, hidden groups included.
    ///
    /// Backs `ServerHandler::get_tool`, which rmcp calls on a FRESH handler
    /// and caches per name process-wide (SEP-2243 `Mcp-Param-*` header
    /// validation): the answer must not depend on a session. Visibility is
    /// enforced by `tools/call` itself.
    pub(crate) fn tool_wire(&self, name: &str) -> Option<Tool> {
        self.all_tools.wire(name)
    }

    pub(crate) fn has_tool_key(&self, name: &str) -> bool {
        self.tool_keys.contains(name)
    }

    pub(crate) fn has_resource_key(&self, uri: &str) -> bool {
        self.resource_keys.contains(uri)
    }

    pub(crate) fn has_template_shape(&self, shape: &str) -> bool {
        self.template_shapes.contains(shape)
    }

    pub(crate) fn has_prompt_key(&self, name: &str) -> bool {
        self.prompt_keys.contains(name)
    }

    pub(crate) fn tool_names(&self) -> Vec<&str> {
        self.members.tools.iter().map(|t| t.key.as_str()).collect()
    }

    pub(crate) fn resource_count(&self) -> usize {
        self.members.resources.len() + self.members.templates.len()
    }

    pub(crate) fn prompt_count(&self) -> usize {
        self.members.prompts.len()
    }

    pub(crate) fn group_names(&self) -> Vec<&str> {
        self.groups.iter().map(|g| g.name.as_ref()).collect()
    }
}

impl Members {
    fn view(
        &self,
        filter: bool,
        enabled: Box<[bool]>,
        private: PrivateMembers,
    ) -> Result<SessionView, String> {
        let private_service = "<session>";
        let mut tools = family_members(&self.tools, &enabled);
        tools.extend(private.tools.iter().map(|route| {
            (
                private_service,
                route.name.to_string(),
                route.requirements,
                route.to_rmcp_tool(),
                Arc::clone(route),
            )
        }));
        let mut resources = family_members(&self.resources, &enabled);
        let mut templates = template_members(&self.templates, &enabled);
        for route in &private.resources {
            if route.is_template() {
                templates.push((
                    private_service,
                    route.requirements,
                    route.to_rmcp_resource_template(),
                    Arc::clone(route),
                ));
            } else {
                resources.push((
                    private_service,
                    route.uri.to_string(),
                    route.requirements,
                    route.to_rmcp_resource(),
                    Arc::clone(route),
                ));
            }
        }
        let mut prompts = family_members(&self.prompts, &enabled);
        prompts.extend(private.prompts.iter().map(|route| {
            (
                private_service,
                route.name.to_string(),
                route.requirements,
                route.to_rmcp_prompt(),
                Arc::clone(route),
            )
        }));
        Ok(SessionView {
            tools: Family::build("tool name", filter, tools)?,
            resources: Family::build("resource URI", filter, resources)?,
            resource_templates: TemplateFamily::build(filter, templates)?,
            prompts: Family::build("prompt name", filter, prompts)?,
            enabled,
            private,
        })
    }
}

fn is_enabled(group: Option<usize>, enabled: &[bool]) -> bool {
    group.is_none_or(|g| enabled[g])
}

fn family_members<R, W: Clone>(
    members: &[Member<R, W>],
    enabled: &[bool],
) -> Vec<FamilyMember<R, W>> {
    members
        .iter()
        .filter(|m| is_enabled(m.group, enabled))
        .map(|m| {
            (
                m.service,
                m.key.clone(),
                m.requirements,
                m.wire.clone(),
                Arc::clone(&m.route),
            )
        })
        .collect()
}

fn template_members(
    members: &[Member<ResourceRoute, ResourceTemplate>],
    enabled: &[bool],
) -> Vec<TemplateMember> {
    members
        .iter()
        .filter(|m| is_enabled(m.group, enabled))
        .map(|m| {
            (
                m.service,
                m.requirements,
                m.wire.clone(),
                Arc::clone(&m.route),
            )
        })
        .collect()
}

fn require_auth_for_scopes(
    auth_enabled: bool,
    kind: &str,
    name: &str,
    requirements: &ToolRequirements,
) {
    if !scopes_allowed(auth_enabled, requirements) {
        panic!(
            "MCP {kind} `{name}` declares OAuth scopes but `mcp.auth` is disabled; \
             configure `mcp.auth.issuer` or remove the scope requirement"
        );
    }
}

/// OAuth scopes are only meaningful with `mcp.auth` on — checked at boot for
/// catalog members and at runtime for session-private ones.
pub(crate) fn scopes_allowed(auth_enabled: bool, requirements: &ToolRequirements) -> bool {
    auth_enabled || (requirements.scopes.is_empty() && requirements.any_scopes.is_empty())
}

/// A prompt's completion providers each name one of its declared
/// arguments, at most once.
pub(crate) fn prompt_completions_valid(prompt: &PromptRoute) -> Result<(), String> {
    let mut seen = HashSet::new();
    for provider in &prompt.completions {
        let argument = provider.argument.as_ref();
        if !prompt.arguments.iter().any(|a| a.name == argument) {
            return Err(format!(
                "MCP prompt `{}` has a completion provider for `{argument}`, which is not \
                 one of its arguments",
                prompt.name
            ));
        }
        if !seen.insert(argument) {
            return Err(format!(
                "MCP prompt `{}` has two completion providers for `{argument}`",
                prompt.name
            ));
        }
    }
    Ok(())
}

/// A resource's completion providers each name one variable of its URI
/// template, at most once — a fixed URI has none to complete.
pub(crate) fn resource_completions_valid(resource: &ResourceRoute) -> Result<(), String> {
    if resource.completions.is_empty() {
        return Ok(());
    }
    if !resource.is_template() {
        return Err(format!(
            "MCP resource `{}` has completion providers but a fixed URI — only URI \
             template variables can be completed",
            resource.uri
        ));
    }
    // An unparsable template is reported by the template checks.
    let Ok(template) = UriTemplate::parse(&resource.uri) else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    for provider in &resource.completions {
        let argument = provider.argument.as_ref();
        if !template.variables().any(|v| v == argument) {
            return Err(format!(
                "MCP resource `{}` has a completion provider for `{argument}`, which is not \
                 a variable of its URI template",
                resource.uri
            ));
        }
        if !seen.insert(argument) {
            return Err(format!(
                "MCP resource `{}` has two completion providers for `{argument}`",
                resource.uri
            ));
        }
    }
    Ok(())
}
