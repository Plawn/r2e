use std::collections::HashSet;

use crate::auth::tools::ToolRequirements;
use crate::route::{PromptRoute, ResourceRoute};
use crate::uri_template::UriTemplate;

pub(super) fn require_auth_for_scopes(
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
