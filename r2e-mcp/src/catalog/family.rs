use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rmcp::model::{Prompt, Resource, ResourceTemplate, Tool};
use rmcp::service::{RequestContext, RoleServer};

use super::principal_of;
use super::view::{Member, PrivateMembers, SessionView};
use crate::auth::tools::{requirements_visible, ToolRequirements};
use crate::pagination::ListKey;
use crate::route::{PromptRoute, ResourceRoute, ToolRoute};
use crate::uri_template::UriTemplate;

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
    pub(crate) fn build(
        kind: &str,
        filter: bool,
        members: Vec<FamilyMember<R, W>>,
    ) -> Result<Self, String> {
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

    /// Append this caller's visible members to `out`, skipping the keys
    /// `shadowed` accepts — how the global registry's list is merged behind
    /// a session's own (a session-private member hides a global namesake).
    pub(crate) fn extend_visible(
        &self,
        context: &RequestContext<RoleServer>,
        out: &mut Vec<W>,
        shadowed: impl Fn(&str) -> bool,
    ) where
        W: ListKey,
    {
        let principal = principal_of(context);
        out.extend(
            self.list
                .iter()
                .zip(self.reqs.iter())
                .filter(|(wire, req)| {
                    !shadowed(wire.list_key())
                        && (!self.filter || requirements_visible(principal, req))
                })
                .map(|(wire, _)| wire.clone()),
        );
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub(crate) fn route(&self, key: &str) -> Option<&Arc<R>> {
        self.routes.get(key).map(|entry| &entry.route)
    }

    pub(crate) fn wire(&self, key: &str) -> Option<W> {
        let entry = self.routes.get(key)?;
        self.list.get(entry.wire_index).cloned()
    }

    /// Whether `other` serves exactly the same members (same keys, same
    /// route objects) — the "did this family's list change" test.
    pub(super) fn same_members(&self, other: &Self) -> bool {
        self.routes.len() == other.routes.len()
            && self.routes.iter().all(|(key, entry)| {
                other
                    .routes
                    .get(key)
                    .is_some_and(|o| Arc::ptr_eq(&o.route, &entry.route))
            })
    }
}

pub(super) struct TemplateFamilyRoute {
    pub(super) matcher: UriTemplate,
    route: Arc<ResourceRoute>,
}

pub(crate) struct TemplateFamily {
    pub(super) routes: Vec<TemplateFamilyRoute>,
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
    pub(crate) fn build(filter: bool, members: Vec<TemplateMember>) -> Result<Self, String> {
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

    /// See [`Family::extend_visible`]; templates are shadowed by their text.
    pub(crate) fn extend_visible(
        &self,
        context: &RequestContext<RoleServer>,
        out: &mut Vec<ResourceTemplate>,
        shadowed: impl Fn(&str) -> bool,
    ) {
        let principal = principal_of(context);
        out.extend(
            self.list
                .iter()
                .zip(self.reqs.iter())
                .filter(|(wire, req)| {
                    !shadowed(&wire.uri_template)
                        && (!self.filter || requirements_visible(principal, req))
                })
                .map(|(wire, _)| wire.clone()),
        );
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Whether a template with exactly this text is served.
    pub(crate) fn has_raw(&self, raw: &str) -> bool {
        self.routes.iter().any(|entry| entry.matcher.raw() == raw)
    }

    pub(crate) fn route(
        &self,
        uri: &str,
    ) -> Option<(&Arc<ResourceRoute>, BTreeMap<String, String>)> {
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
    pub(crate) fn template(&self, raw: &str) -> Option<&Arc<ResourceRoute>> {
        if let Some(entry) = self.routes.iter().find(|e| e.matcher.raw() == raw) {
            return Some(&entry.route);
        }
        let shape = UriTemplate::parse(raw).ok()?.shape();
        self.routes
            .iter()
            .find(|e| e.matcher.shape() == shape)
            .map(|e| &e.route)
    }

    pub(super) fn same_members(&self, other: &Self) -> bool {
        self.routes.len() == other.routes.len()
            && self
                .routes
                .iter()
                .zip(other.routes.iter())
                .all(|(a, b)| Arc::ptr_eq(&a.route, &b.route))
    }
}

/// The members of a view before they are folded into families.
#[derive(Default)]
pub(super) struct FamilyInputs {
    pub(super) tools: Vec<FamilyMember<ToolRoute, Tool>>,
    pub(super) resources: Vec<FamilyMember<ResourceRoute, Resource>>,
    pub(super) templates: Vec<TemplateMember>,
    pub(super) prompts: Vec<FamilyMember<PromptRoute, Prompt>>,
}

impl FamilyInputs {
    /// Append `private`'s members, owned by `label` for diagnostics.
    pub(super) fn extend_private(&mut self, label: &'static str, private: &PrivateMembers) {
        self.tools.extend(private.tools.iter().map(|route| {
            (
                label,
                route.name.to_string(),
                route.requirements,
                route.to_rmcp_tool(),
                Arc::clone(route),
            )
        }));
        for route in &private.resources {
            if route.is_template() {
                self.templates.push((
                    label,
                    route.requirements,
                    route.to_rmcp_resource_template(),
                    Arc::clone(route),
                ));
            } else {
                self.resources.push((
                    label,
                    route.uri.to_string(),
                    route.requirements,
                    route.to_rmcp_resource(),
                    Arc::clone(route),
                ));
            }
        }
        self.prompts.extend(private.prompts.iter().map(|route| {
            (
                label,
                route.name.to_string(),
                route.requirements,
                route.to_rmcp_prompt(),
                Arc::clone(route),
            )
        }));
    }

    pub(super) fn into_view(
        self,
        filter: bool,
        enabled: Box<[bool]>,
        private: PrivateMembers,
    ) -> Result<SessionView, String> {
        Ok(SessionView {
            tools: Family::build("tool name", filter, self.tools)?,
            resources: Family::build("resource URI", filter, self.resources)?,
            resource_templates: TemplateFamily::build(filter, self.templates)?,
            prompts: Family::build("prompt name", filter, self.prompts)?,
            enabled,
            private,
        })
    }
}

fn is_enabled(group: Option<usize>, enabled: &[bool]) -> bool {
    group.is_none_or(|g| enabled[g])
}

pub(super) fn family_members<R, W: Clone>(
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

pub(super) fn template_members(
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
