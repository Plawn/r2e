//! HTTP Basic authentication for the MCP endpoint (`mcp.auth.basic.*`).
//!
//! One static credential pair for deployments where an OAuth issuer is
//! overkill: a private agent runtime, a gateway that already terminates the
//! user-facing auth, a staging box. It composes with the OAuth layer — the
//! `Authorization` scheme selects the path per request — or stands alone
//! (no `issuer` ⇒ no PRM document, no discovery, no validator bean work).
//!
//! The principal is built ONCE at boot: every authenticated request costs a
//! base64 decode, two constant-time compares and an `Arc` clone. Scope and
//! role checks (`required-scopes`, `#[tool(scopes)]`, `#[roles]`) see the
//! configured `scopes`/`roles` exactly as they would see a token's.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use r2e_core::plugin::PluginBuildError;
use r2e_core::StandardClaims;
use r2e_security::AuthenticatedUser;
use subtle::ConstantTimeEq;

use super::config::McpBasicAuthConfig;
use super::error::McpAuthError;
use super::validator::{hash_token, McpPrincipal};

/// The realm advertised when `mcp.auth.basic.realm` is unset.
pub(crate) const DEFAULT_REALM: &str = "mcp";

/// The boot-resolved Basic credential pair and the principal it maps to.
pub(crate) struct BasicCredentials {
    username: Box<[u8]>,
    password: Box<[u8]>,
    realm: Arc<str>,
    principal: McpPrincipal,
}

impl BasicCredentials {
    /// Validate the section and prebuild the principal.
    pub(crate) fn from_config(cfg: &McpBasicAuthConfig) -> Result<Self, PluginBuildError> {
        let username = cfg.username.as_str();
        if username.is_empty() {
            return Err("mcp.auth.basic.username must not be empty".into());
        }
        if username.contains(':') {
            // RFC 7617 §2: the user-id cannot contain a colon — it is the
            // separator on the wire, so such a pair could never match.
            return Err(format!("mcp.auth.basic.username `{username}` must not contain ':'").into());
        }
        if cfg.password.is_empty() {
            return Err("mcp.auth.basic.password must not be empty".into());
        }
        let realm = cfg.realm.as_deref().unwrap_or(DEFAULT_REALM);
        if realm.is_empty() || realm.chars().any(char::is_control) {
            return Err("mcp.auth.basic.realm must be a non-empty printable string".into());
        }

        let roles = cfg.roles.clone().unwrap_or_default();
        let scopes: Arc<[String]> = cfg.scopes.clone().unwrap_or_default().into();
        let claims = StandardClaims {
            sub: username.to_string(),
            preferred_username: Some(username.to_string()),
            scope: (!scopes.is_empty()).then(|| scopes.join(" ")),
            ..StandardClaims::default()
        };
        let user = Arc::new(AuthenticatedUser {
            sub: username.to_string(),
            email: None,
            roles,
            claims,
        });
        let principal = McpPrincipal {
            user,
            scopes,
            // The same stable, non-reversible correlation key bearer tokens
            // get — never the credential bytes.
            token_hash: hash_token(&format!("{username}:{}", cfg.password)),
        };
        Ok(Self {
            username: username.as_bytes().into(),
            password: cfg.password.as_bytes().into(),
            realm: realm.into(),
            principal,
        })
    }

    /// The realm carried in the `WWW-Authenticate: Basic` challenge.
    pub(crate) fn realm(&self) -> &Arc<str> {
        &self.realm
    }

    /// Check `<credentials>` (the base64 part of `Authorization: Basic …`).
    pub(crate) fn authenticate(&self, encoded: &str) -> Result<McpPrincipal, McpAuthError> {
        const MALFORMED: McpAuthError = McpAuthError::InvalidToken("malformed basic credentials");
        let decoded = STANDARD.decode(encoded).map_err(|_| MALFORMED)?;
        let Some(sep) = decoded.iter().position(|&b| b == b':') else {
            return Err(MALFORMED);
        };
        let (user, pass) = (&decoded[..sep], &decoded[sep + 1..]);
        // Both compares always run and are combined without a branch: a
        // wrong user-id costs exactly what a wrong password costs.
        let matched = self.username.ct_eq(user) & self.password.ct_eq(pass);
        if bool::from(matched) {
            Ok(self.principal.clone())
        } else {
            Err(McpAuthError::InvalidToken("invalid credentials"))
        }
    }
}

/// `Authorization: Basic <credentials>` → `<credentials>`; `None` for any
/// other scheme (matched case-insensitively, RFC 9110 §11.1).
pub(crate) fn basic_credentials(header: &str) -> Option<&str> {
    let (scheme, rest) = header.trim_start().split_once(' ')?;
    scheme.eq_ignore_ascii_case("Basic").then(|| rest.trim())
}
