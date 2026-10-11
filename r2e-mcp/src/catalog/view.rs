use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::model::{Prompt, Resource, ResourceTemplate, Tool};

use super::family::{family_members, template_members, Family, FamilyInputs, TemplateFamily};
use crate::auth::tools::ToolRequirements;
use crate::route::{PromptRoute, ResourceRoute, ToolRoute};

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
    /// A view of `members` alone, with no catalog behind it: the global
    /// registry's view (`label` names it in duplicate diagnostics).
    pub(crate) fn standalone(
        label: &'static str,
        filter: bool,
        members: PrivateMembers,
    ) -> Result<SessionView, String> {
        let mut inputs = FamilyInputs::default();
        inputs.extend_private(label, &members);
        inputs.into_view(filter, Box::default(), members)
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
        enabled: Box<[bool]>,
        private: PrivateMembers,
    ) -> Result<SessionView, String> {
        let mut inputs = FamilyInputs {
            tools: family_members(&self.tools, &enabled),
            resources: family_members(&self.resources, &enabled),
            templates: template_members(&self.templates, &enabled),
            prompts: family_members(&self.prompts, &enabled),
        };
        inputs.extend_private("<session>", &private);
        inputs.into_view(filter, enabled, private)
    }
}
