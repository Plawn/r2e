//! Per-tool authorization: scope requirements checked against the validated
//! principal, and the `tools/list` visibility filter.
//!
//! Role requirements (`#[roles]`/`#[all_roles]`) are enforced by the shared
//! guard machinery (`r2e_security::RolesGuard` over the identity in the
//! request extensions); they are additionally RECORDED here so `tools/list`
//! can hide tools the caller cannot invoke.

use r2e_core::http::Extensions;

use super::validator::McpPrincipal;
use crate::error::McpError;

/// The authorization requirements of one tool, emitted by `#[mcp_routes]`
/// from `#[tool(scopes/any_scopes)]` + `#[roles]`/`#[all_roles]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolRequirements {
    /// Scopes the caller must ALL hold (`#[tool(scopes = "a,b")]`).
    pub scopes: &'static [&'static str],
    /// Scopes of which the caller must hold AT LEAST ONE
    /// (`#[tool(any_scopes = "a,b")]`).
    pub any_scopes: &'static [&'static str],
    /// Roles of which the caller must hold at least one (`#[roles]`) —
    /// enforced by the guard, recorded for `tools/list` filtering.
    pub roles: &'static [&'static str],
    /// Roles the caller must ALL hold (`#[all_roles]`) — enforced by the
    /// guard, recorded for `tools/list` filtering.
    pub all_roles: &'static [&'static str],
}

impl ToolRequirements {
    /// A tool with no authorization requirements.
    pub const NONE: ToolRequirements = ToolRequirements {
        scopes: &[],
        any_scopes: &[],
        roles: &[],
        all_roles: &[],
    };

    /// Whether the tool has no requirements at all.
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
            && self.any_scopes.is_empty()
            && self.roles.is_empty()
            && self.all_roles.is_empty()
    }

    fn missing_scopes(&self, principal: &McpPrincipal) -> Vec<&'static str> {
        self.scopes
            .iter()
            .filter(|s| !principal.has_scope(s))
            .copied()
            .collect()
    }

    fn any_scope_ok(&self, principal: &McpPrincipal) -> bool {
        self.any_scopes.is_empty() || self.any_scopes.iter().any(|s| principal.has_scope(s))
    }

    fn roles_ok(&self, principal: &McpPrincipal) -> bool {
        let user = &principal.user;
        (self.roles.is_empty() || self.roles.iter().any(|r| user.has_role(r)))
            && self.all_roles.iter().all(|r| user.has_role(r))
    }

    /// Whether the principal satisfies every requirement (scopes AND
    /// roles). Used by the `tools/list` visibility filter.
    pub fn satisfied_by(&self, principal: &McpPrincipal) -> bool {
        self.missing_scopes(principal).is_empty()
            && self.any_scope_ok(principal)
            && self.roles_ok(principal)
    }
}

/// Check a member's scope requirements against the caller. `kind` names the
/// member family in denial messages (`"tool"` / `"resource"` / `"prompt"`).
/// Role requirements are enforced separately by the shared guard machinery.
pub fn check_access(
    extensions: Option<&Extensions>,
    kind: &'static str,
    name: &str,
    req: &ToolRequirements,
) -> Result<(), McpError> {
    if req.scopes.is_empty() && req.any_scopes.is_empty() {
        return Ok(());
    }
    let Some(principal) = extensions.and_then(|ext| ext.get::<McpPrincipal>()) else {
        tracing::error!(
            kind,
            name,
            "member has scope requirements but no authenticated principal reached it \
             (auth layer bypassed?); denying"
        );
        return Err(McpError::Unauthorized(format!(
            "{kind} `{name}` requires an authenticated caller"
        )));
    };

    let missing = req.missing_scopes(principal);
    if !missing.is_empty() {
        return Err(McpError::Forbidden(format!(
            "{kind} `{name}` requires scope(s) `{}` that the token does not carry; \
             re-authorize requesting them",
            missing.join(", ")
        )));
    }
    if !req.any_scope_ok(principal) {
        return Err(McpError::Forbidden(format!(
            "{kind} `{name}` requires at least one of the scopes `{}`; \
             re-authorize requesting one",
            req.any_scopes.join(", ")
        )));
    }
    Ok(())
}

/// Visibility check when the caller principal has already been extracted.
/// List handlers use this form so one request performs one extensions lookup,
/// not one lookup per registered member.
pub(crate) fn requirements_visible(
    principal: Option<&McpPrincipal>,
    req: &ToolRequirements,
) -> bool {
    if req.is_empty() {
        return true;
    }
    match principal {
        Some(principal) => req.satisfied_by(principal),
        None => false,
    }
}
