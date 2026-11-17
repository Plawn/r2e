//! The declared public clients (`mcp.auth.public-client-id` /
//! `mcp.auth.public-clients`), normalized once at boot, and the two things
//! built from them:
//!
//! - the DCR shim's client selection — which pre-registered client id a
//!   registration call is answered with ([`PublicClients::select`]);
//! - the validation-side [`ClientPolicy`] — binding a token to the client it
//!   was issued to (`enforce-azp`) and capping what that client may do
//!   (`allowed-scopes`).

use std::sync::Arc;

use r2e_core::plugin::PluginBuildError;

use super::config::{McpAuthConfig, TokenValidationMode};
use super::error::{AuthFailure, McpAuthError};
use super::shim::{redirect_allowed, DEFAULT_REDIRECT_ALLOWLIST};
use super::validator::McpPrincipal;

/// One declared public client.
#[derive(Debug)]
pub(crate) struct PublicClient {
    pub client_id: String,
    /// Redirect-URI patterns this client answers for.
    pub redirect_allowlist: Vec<String>,
    /// The `scope` the registration answer advertises, space-joined.
    pub registration_scope: String,
    /// Cap on a token's effective scopes (`None` = no restriction).
    pub allowed_scopes: Option<Arc<[String]>>,
}

/// The declared clients, in configuration order.
#[derive(Debug)]
pub(crate) struct PublicClients {
    pub clients: Vec<PublicClient>,
    /// The single-client form (`public-client-id`): redirect URIs off the
    /// allowlist are dropped from the answer instead of failing the call.
    pub legacy: bool,
}

impl PublicClients {
    /// The first client whose allowlist covers EVERY requested redirect URI.
    pub(crate) fn select(&self, requested: &[&str]) -> Option<&PublicClient> {
        if requested.is_empty() {
            return None;
        }
        self.clients.iter().find(|client| {
            requested
                .iter()
                .all(|uri| redirect_allowed(&client.redirect_allowlist, uri))
        })
    }

    pub(crate) fn ids(&self) -> Vec<String> {
        self.clients.iter().map(|c| c.client_id.clone()).collect()
    }
}

fn scope_key(index: usize, field: &str) -> String {
    format!("public-clients[{index}].{field}")
}

/// Normalize the two configuration forms into one list.
///
/// `validate_scopes` is the caller's RFC 6749 scope-token check;
/// `scopes_supported` is what the protected-resource metadata advertises.
pub(crate) fn resolve_public_clients(
    cfg: &McpAuthConfig,
    scopes_supported: &[String],
    validate_scopes: impl Fn(&str, Option<&[String]>) -> Result<(), PluginBuildError>,
) -> Result<Option<PublicClients>, PluginBuildError> {
    let Some(entries) = &cfg.public_clients else {
        return Ok(cfg.public_client_id.as_ref().map(|client_id| PublicClients {
            clients: vec![PublicClient {
                client_id: client_id.clone(),
                redirect_allowlist: cfg.redirect_uri_allowlist.clone().unwrap_or_else(|| {
                    DEFAULT_REDIRECT_ALLOWLIST
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                }),
                registration_scope: scopes_supported.join(" "),
                allowed_scopes: None,
            }],
            legacy: true,
        }));
    };

    if cfg.public_client_id.is_some() {
        return Err(
            "mcp.auth: `public-client-id` and `public-clients` are both set — keep one. \
             `public-client-id` is the single-client form; to declare several clients, \
             move it into `public-clients` as an entry with its own \
             `redirect-uri-allowlist`"
                .to_string()
                .into(),
        );
    }
    if cfg.redirect_uri_allowlist.is_some() {
        return Err(
            "mcp.auth.redirect-uri-allowlist only applies to `public-client-id`; with \
             `public-clients`, each entry carries its own `redirect-uri-allowlist`"
                .to_string()
                .into(),
        );
    }
    if entries.is_empty() {
        return Err(
            "mcp.auth.public-clients is empty — declare at least one client, or remove \
             the key"
                .to_string()
                .into(),
        );
    }

    let mut clients: Vec<PublicClient> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let client_id = entry.client_id.trim();
        if client_id.is_empty() {
            return Err(format!("mcp.auth.public-clients[{index}].client-id is empty").into());
        }
        if clients.iter().any(|c| c.client_id == client_id) {
            return Err(format!(
                "mcp.auth.public-clients: client id `{client_id}` is declared twice — \
                 merge the two entries"
            )
            .into());
        }
        if entry.redirect_uri_allowlist.is_empty()
            || entry.redirect_uri_allowlist.iter().any(|p| p.is_empty())
        {
            // Fail closed: no implicit default list in this form — a client
            // that answers for nothing (or for an empty pattern) is a typo.
            return Err(format!(
                "mcp.auth.public-clients[{index}] (`{client_id}`): \
                 `redirect-uri-allowlist` is required and its entries must be non-empty \
                 — list the redirect URIs this client is registered with on the IdP"
            )
            .into());
        }
        validate_scopes(&scope_key(index, "scopes"), entry.scopes.as_deref())?;
        validate_scopes(
            &scope_key(index, "allowed-scopes"),
            entry.allowed_scopes.as_deref(),
        )?;
        if let (Some(scopes), Some(allowed)) = (&entry.scopes, &entry.allowed_scopes)
            && let Some(extra) = scopes.iter().find(|s| !allowed.contains(s))
        {
            return Err(format!(
                "mcp.auth.public-clients[{index}] (`{client_id}`): scope `{extra}` is \
                 advertised in `scopes` but not in `allowed-scopes` — a token carrying \
                 it would have it stripped"
            )
            .into());
        }
        for pattern in &entry.redirect_uri_allowlist {
            if let Some(earlier) = clients
                .iter()
                .find(|c| c.redirect_allowlist.contains(pattern))
            {
                tracing::warn!(
                    pattern = %pattern,
                    client_id = %earlier.client_id,
                    shadowed_client_id = %client_id,
                    "mcp.auth.public-clients: the same redirect-URI pattern is listed \
                     under two clients — the first entry always wins for it"
                );
            }
        }
        let registration_scope = match (&entry.scopes, &entry.allowed_scopes) {
            (Some(scopes), _) => scopes.join(" "),
            (None, Some(allowed)) => scopes_supported
                .iter()
                .filter(|s| allowed.contains(s))
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            (None, None) => scopes_supported.join(" "),
        };
        clients.push(PublicClient {
            client_id: client_id.to_string(),
            redirect_allowlist: entry.redirect_uri_allowlist.clone(),
            registration_scope,
            allowed_scopes: entry.allowed_scopes.clone().map(Into::into),
        });
    }
    Ok(Some(PublicClients {
        clients,
        legacy: false,
    }))
}

/// What the layer enforces on a validated bearer token, beyond the
/// validator's own checks. Applied between validation and
/// `required-scopes`, on the HTTP and loopback bearer paths alike (never on
/// HTTP Basic or a trusted loopback principal: no token, no client).
#[derive(Clone, Debug, Default)]
pub(crate) struct ClientPolicy {
    /// `mcp.auth.enforce-azp`.
    enforce_azp: bool,
    /// `(client id, allowed-scopes)` of every declared client.
    clients: Arc<[DeclaredClient]>,
}

/// `(client id, allowed-scopes)`.
type DeclaredClient = (String, Option<Arc<[String]>>);

impl ClientPolicy {
    /// Build the policy, rejecting the combinations that cannot be enforced.
    pub(crate) fn from_config(
        cfg: &McpAuthConfig,
        clients: Option<&PublicClients>,
        validation_mode: TokenValidationMode,
    ) -> Result<Self, PluginBuildError> {
        let enforce_azp = cfg.enforce_azp.unwrap_or(false);
        let declared: Arc<[DeclaredClient]> = clients
            .map(|c| {
                c.clients
                    .iter()
                    .map(|c| (c.client_id.clone(), c.allowed_scopes.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let caps_scopes = declared.iter().any(|(_, allowed)| allowed.is_some());
        if enforce_azp && declared.is_empty() {
            return Err(
                "mcp.auth.enforce-azp: true requires declared clients — set \
                 `mcp.auth.public-clients` (or `mcp.auth.public-client-id`)"
                    .to_string()
                    .into(),
            );
        }
        if validation_mode == TokenValidationMode::Userinfo && (enforce_azp || caps_scopes) {
            let key = if enforce_azp {
                "enforce-azp"
            } else {
                "public-clients[].allowed-scopes"
            };
            return Err(format!(
                "mcp.auth.{key} cannot be enforced with `token-validation: userinfo` \
                 (the userinfo response does not name the client a token was issued \
                 to) — remove the key or use `jwt`/`introspection` validation"
            )
            .into());
        }
        if caps_scopes && !enforce_azp {
            tracing::warn!(
                "mcp.auth.public-clients: `allowed-scopes` is set without \
                 `mcp.auth.enforce-azp: true` — a token carrying no `azp`/`client_id` \
                 claim, or one issued to an undeclared client, is NOT restricted"
            );
        }
        Ok(Self {
            enforce_azp,
            clients: declared,
        })
    }

    /// Apply the policy to a freshly validated principal: refuse a token
    /// from an undeclared client (`enforce-azp`), then narrow its scopes to
    /// the client's `allowed-scopes`.
    pub(crate) fn apply(&self, mut principal: McpPrincipal) -> Result<McpPrincipal, AuthFailure> {
        if self.clients.is_empty() {
            return Ok(principal);
        }
        let declared = principal
            .client_id()
            .and_then(|id| self.clients.iter().find(|(declared, _)| declared == id));
        match declared {
            Some((_, Some(allowed))) => {
                if principal.scopes.iter().any(|s| !allowed.contains(s)) {
                    principal.scopes = principal
                        .scopes
                        .iter()
                        .filter(|s| allowed.contains(s))
                        .cloned()
                        .collect();
                }
            }
            Some((_, None)) => {}
            None if self.enforce_azp => {
                let failure = AuthFailure::new(
                    McpAuthError::InvalidToken("token was not issued to a declared client"),
                    "claims",
                );
                return Err(match principal.client_id() {
                    Some(id) => failure.with_detail(format!("azp `{id}` is not a declared client")),
                    None => failure.with_detail("token carries no `azp`/`client_id` claim"),
                });
            }
            None => {}
        }
        Ok(principal)
    }
}
