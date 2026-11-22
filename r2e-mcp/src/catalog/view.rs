use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::model::{Prompt, Resource, ResourceTemplate, Tool};

use super::family::{family_members, template_members, Family, FamilyInputs, TemplateFamily};
use super::CatalogKeys;
use crate::auth::tools::ToolRequirements;
use crate::route::{McpGroup, PromptRoute, ResourceRoute, ToolRoute};

/// A catalog member: the route plus what a view rebuild needs.
pub(super) struct Member<R, W> {
    pub(super) service: &'static str,
    pub(super) key: String,
    pub(super) requirements: ToolRequirements,
    pub(super) wire: W,
    pub(super) route: Arc<R>,
    pub(super) group: Option<usize>,
}

pub(super) struct GroupDef {
    pub(super) name: Cow<'static, str>,
    pub(super) opt_in: bool,
    /// The service that first declared the group (boot diagnostics).
    pub(super) owner: &'static str,
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

    /// The group of every grouped member (repeats included).
    pub(crate) fn groups(&self) -> impl Iterator<Item = &McpGroup> {
        let tools = self.tools.iter().filter_map(|t| t.group.as_ref());
        let resources = self.resources.iter().filter_map(|r| r.group.as_ref());
        let prompts = self.prompts.iter().filter_map(|p| p.group.as_ref());
        tools.chain(resources).chain(prompts)
    }

    /// The declaration of the group `name`, when a member carries it.
    pub(crate) fn group(&self, name: &str) -> Option<&McpGroup> {
        self.groups().find(|group| group.name == name)
    }

    /// Whether any member is in a group.
    pub(crate) fn grouped(&self) -> bool {
        self.groups().next().is_some()
    }
}

/// Whether `group` is on for a session whose state is `enabled` (catalog
/// groups, by index) and `dynamic` (explicit states of the groups only
/// dynamic members declare): a catalog group follows the catalog's switch,
/// any other its explicit state, else its own `opt_in` default.
pub(crate) fn group_on(
    keys: &CatalogKeys,
    enabled: &[bool],
    dynamic: &BTreeMap<String, bool>,
    group: &McpGroup,
) -> bool {
    if let Some(index) = keys.group(&group.name) {
        return enabled[index];
    }
    dynamic
        .get(group.name.as_ref())
        .copied()
        .unwrap_or(!group.opt_in)
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

    pub(crate) fn or(self, other: Changed) -> Changed {
        Changed {
            tools: self.tools || other.tools,
            resources: self.resources || other.resources,
            prompts: self.prompts || other.prompts,
        }
    }
}

/// What one session serves.
pub(crate) struct SessionView {
    /// Per catalog group (same index as the catalog's group table).
    pub(crate) enabled: Box<[bool]>,
    /// Explicit on/off states of the groups only dynamic members declare
    /// (session-private or registry), by name. A group absent from here
    /// follows its `opt_in` default — see [`group_on`].
    pub(crate) dynamic: BTreeMap<String, bool>,
    /// Whether a private member is in a group. On the registry's view this
    /// is what makes its members gated at request time.
    pub(crate) grouped: bool,
    pub(crate) private: PrivateMembers,
    pub(crate) tools: Family<ToolRoute, Tool>,
    pub(crate) resources: Family<ResourceRoute, Resource>,
    pub(crate) resource_templates: TemplateFamily,
    pub(crate) prompts: Family<PromptRoute, Prompt>,
}

impl SessionView {
    /// A view of `members` alone, with no catalog behind it: the global
    /// registry's view (`label` names it in duplicate diagnostics).
    pub(crate) fn standalone(
        label: &'static str,
        filter: bool,
        members: PrivateMembers,
    ) -> Result<SessionView, String> {
        let mut inputs = FamilyInputs::default();
        // Every member is in: a registry member's group is evaluated
        // against the asking session at request time.
        inputs.extend_private(label, &members, |_| true);
        inputs.into_view(filter, Box::default(), BTreeMap::new(), members)
    }

    /// Whether `group` is on for the session this view belongs to.
    pub(crate) fn group_on(&self, keys: &CatalogKeys, group: &McpGroup) -> bool {
        group_on(keys, &self.enabled, &self.dynamic, group)
    }

    /// The families in which `registry` serves a member whose group is not
    /// in the same state for `self` and `next` — the registry's members are
    /// gated at request time, so no family rebuild shows that change.
    pub(crate) fn registry_diff(
        &self,
        next: &SessionView,
        keys: &CatalogKeys,
        registry: &SessionView,
    ) -> Changed {
        if !registry.grouped {
            return Changed::default();
        }
        let flipped = |group: &Option<McpGroup>| {
            group
                .as_ref()
                .is_some_and(|g| self.group_on(keys, g) != next.group_on(keys, g))
        };
        let members = &registry.private;
        Changed {
            tools: members.tools.iter().any(|t| flipped(&t.group)),
            resources: members.resources.iter().any(|r| flipped(&r.group)),
            prompts: members.prompts.iter().any(|p| flipped(&p.group)),
        }
    }

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
pub(super) struct Members {
    pub(super) tools: Vec<Member<ToolRoute, Tool>>,
    pub(super) resources: Vec<Member<ResourceRoute, Resource>>,
    pub(super) templates: Vec<Member<ResourceRoute, ResourceTemplate>>,
    pub(super) prompts: Vec<Member<PromptRoute, Prompt>>,
}

impl Members {
    pub(super) fn view(
        &self,
        filter: bool,
        keys: Option<&CatalogKeys>,
        enabled: Box<[bool]>,
        dynamic: BTreeMap<String, bool>,
        private: PrivateMembers,
    ) -> Result<SessionView, String> {
        let mut inputs = FamilyInputs {
            tools: family_members(&self.tools, &enabled),
            resources: family_members(&self.resources, &enabled),
            templates: template_members(&self.templates, &enabled),
            prompts: family_members(&self.prompts, &enabled),
        };
        // A session-private member of a group that is off is left out here
        // (the view is rebuilt on every group change).
        inputs.extend_private("<session>", &private, |group| {
            keys.is_none_or(|keys| group_on(keys, &enabled, &dynamic, group))
        });
        inputs.into_view(filter, enabled, dynamic, private)
    }
}
