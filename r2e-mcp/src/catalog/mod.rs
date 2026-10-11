//! The boot-time member catalog and the per-session views derived from it.
//!
//! [`Catalog`] is immutable: every `#[mcp_routes]` member of every registered
//! service, with its group. A [`SessionView`] is what one session serves —
//! the catalog members of its enabled groups plus its session-private
//! members — with the wire lists precomputed. Sessions that never reshape
//! their list share [`Catalog::default_view`], so the common case costs one
//! `Arc` clone per session. The global registry
//! ([`McpRegistry`](crate::McpRegistry)) keeps a third kind of view — a
//! [`SessionView::standalone`] with no catalog groups — merged with the
//! session's at request time (see `SessionState` in `session/state.rs`).

mod family;
mod validate;
mod view;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use r2e_core::http::Parts;
use rmcp::model::{
    Implementation, PromptsCapability, ResourcesCapability, ServerCapabilities, ServerInfo, Tool,
    ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};

use crate::auth::McpPrincipal;
use crate::registry::RegisteredMcpService;
use crate::route::{McpGroup, ToolRoute};

use family::{family_members, template_members, Family, TemplateFamily};
use validate::require_auth_for_scopes;
pub(crate) use validate::{prompt_completions_valid, resource_completions_valid, scopes_allowed};
pub(crate) use view::{Changed, PrivateMembers, SessionView};
use view::{GroupDef, Member, Members};

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
}

/// The auth layer's principal, carried in the HTTP request parts that the
/// transport copies into the request extensions.
pub(crate) fn principal_in(parts: Option<&Parts>) -> Option<&McpPrincipal> {
    parts.and_then(|parts| parts.extensions.get::<McpPrincipal>())
}

fn principal_of(context: &RequestContext<RoleServer>) -> Option<&McpPrincipal> {
    principal_in(context.extensions.get::<Parts>())
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

        // Every list is dynamic: the `McpRegistry` bean can add a tool, a
        // resource (with completion providers) or a prompt to every session
        // at any time, so all four capabilities are advertised with
        // `listChanged` from the start — a client that only reads
        // capabilities at `initialize` would otherwise never re-list.
        let mut info = ServerInfo::default();
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        // The typestate builder cannot set `list_changed`; set the fields.
        let mut tools_capability = ToolsCapability::default();
        tools_capability.list_changed = Some(true);
        capabilities.tools = Some(tools_capability);
        let mut resource_capabilities = ResourcesCapability::default();
        resource_capabilities.subscribe = Some(true);
        resource_capabilities.list_changed = Some(true);
        capabilities.resources = Some(resource_capabilities);
        let mut prompt_capabilities = PromptsCapability::default();
        prompt_capabilities.list_changed = Some(true);
        capabilities.prompts = Some(prompt_capabilities);
        capabilities.completions = Some(Default::default());
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

    /// `mcp.auth.filter-members` (and auth on): per-caller list filtering.
    pub(crate) fn filter(&self) -> bool {
        self.filter
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
