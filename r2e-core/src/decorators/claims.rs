//! Typed JWT / OIDC claim set carried by authenticated identities.
//!
//! [`Identity::claims`](crate::Identity::claims) returns a [`StandardClaims`]
//! rather than an open `serde_json::Value`: a validated token is deserialized
//! **once**, straight into this struct, and the fields every consumer actually
//! reads (`sub`, `email`, roles, Keycloak's `realm_access` / `resource_access`)
//! become field reads instead of per-request tree walks.
//!
//! Anything the struct does not name lands in [`StandardClaims::extra`] and is
//! reachable with [`StandardClaims::get`], so provider- or app-specific claims
//! are never lost.
//!
//! The type lives in `r2e-core` (not `r2e-security`) because the `Identity`
//! trait is declared here and must name it; it carries no security logic — it
//! is a plain `serde` struct.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The `aud` (audience) claim: either a single string or an array of strings.
///
/// RFC 7519 allows both shapes, so the enum is `#[serde(untagged)]` and
/// round-trips to whichever form the issuer used.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum Audience {
    /// A single audience: `"aud": "my-api"`.
    Single(String),
    /// Several audiences: `"aud": ["my-api", "other-api"]`.
    Multiple(Vec<String>),
}

impl Audience {
    /// Iterate over the audience values, whichever shape the claim used.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        match self {
            Audience::Single(one) => std::slice::from_ref(one),
            Audience::Multiple(many) => many.as_slice(),
        }
        .iter()
        .map(String::as_str)
    }

    /// Whether `audience` is one of the values carried by this claim.
    pub fn contains(&self, audience: &str) -> bool {
        self.iter().any(|a| a == audience)
    }

    /// The single audience value, or `None` when the claim is an array.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Audience::Single(one) => Some(one.as_str()),
            Audience::Multiple(_) => None,
        }
    }
}

/// Keycloak's `realm_access` claim — realm-level roles.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RealmAccess {
    /// Roles granted at the realm level.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// One entry of Keycloak's `resource_access` map — roles for a single client.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientAccess {
    /// Roles granted for this client.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The claim set of a validated JWT.
///
/// Known claims are typed fields; everything else is captured in
/// [`extra`](Self::extra) and read through [`get`](Self::get).
///
/// # `sub` and `Default`
///
/// `sub` is `#[serde(default)]` so a token without a subject still
/// deserializes and is rejected by the validator with a precise error
/// (`Token has no 'sub' (subject) claim`) instead of a generic serde failure.
/// Every claim set that reaches application code through
/// `JwtClaimsValidator::validate` therefore has a **non-empty** `sub`; the
/// empty default only appears on hand-built values.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StandardClaims {
    /// `sub` — the subject (unique principal identifier).
    #[serde(default)]
    pub sub: String,

    /// `email`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// `exp` — expiry, seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,

    /// `iat` — issued-at, seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,

    /// `nbf` — not-before, seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,

    /// `iss` — the issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,

    /// `aud` — the audience(s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<Audience>,

    /// `preferred_username` (OIDC profile scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_username: Option<String>,

    /// `name` (OIDC profile scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// `scope` — space-separated OAuth scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// `roles` — the plain OIDC role array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,

    /// `realm_access` — Keycloak realm roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_access: Option<RealmAccess>,

    /// `resource_access` — Keycloak per-client roles, keyed by client id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_access: Option<HashMap<String, ClientAccess>>,

    /// Every claim not named above, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl StandardClaims {
    /// Look up a **custom** claim by name.
    ///
    /// Only [`extra`](Self::extra) is searched: known claims are typed fields
    /// (`claims.sub`, `claims.email`, `claims.roles`, …) and are deliberately
    /// **not** reachable here.
    ///
    /// ```ignore
    /// let tenant = claims.get("tenant_id").and_then(|v| v.as_str());
    /// ```
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.extra.get(key)
    }

    /// Keycloak realm roles (`realm_access.roles`), empty when absent.
    pub fn realm_roles(&self) -> &[String] {
        self.realm_access
            .as_ref()
            .map(|r| r.roles.as_slice())
            .unwrap_or_default()
    }

    /// Keycloak client roles (`resource_access.<client_id>.roles`), empty when absent.
    pub fn client_roles(&self, client_id: &str) -> &[String] {
        self.resource_access
            .as_ref()
            .and_then(|m| m.get(client_id))
            .map(|c| c.roles.as_slice())
            .unwrap_or_default()
    }

    /// The individual OAuth scopes carried by the space-separated `scope` claim.
    pub fn scopes(&self) -> impl Iterator<Item = &str> {
        self.scope.as_deref().unwrap_or_default().split_whitespace()
    }
}
