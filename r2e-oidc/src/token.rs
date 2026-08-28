use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, Header};
use serde::{Deserialize, Serialize};

use crate::config::OidcServerConfig;
use crate::error::OidcError;
use crate::keys::OidcKeyPair;
use crate::store::OidcUser;

/// Service for signing JWT tokens.
pub(crate) struct TokenService {
    key_pair: Arc<OidcKeyPair>,
    config: OidcServerConfig,
    issuer: String,
}

impl TokenService {
    pub fn new(key_pair: Arc<OidcKeyPair>, config: OidcServerConfig, issuer: String) -> Self {
        Self {
            key_pair,
            config,
            issuer,
        }
    }

    /// Issue a JWT for the given user and the server's configured audience.
    pub fn issue_user_token(&self, user: &OidcUser, scope: &str) -> Result<String, OidcError> {
        let (iat, exp) = self.timestamps()?;

        let claims = AccessTokenClaims {
            sub: user.sub.clone(),
            iss: self.issuer.clone(),
            aud: self.config.audience.clone(),
            iat,
            exp,
            roles: user.roles.clone(),
            email: user.email.clone(),
            scope: scope.to_string(),
            token_use: "access".into(),
            principal_type: "user".into(),
            client_id: None,
            extra: filter_extra_claims(&user.extra_claims),
        };

        self.sign(&claims)
    }

    /// Issue a JWT for a client_credentials grant.
    pub fn issue_client_token(&self, client_id: &str, scope: &str) -> Result<String, OidcError> {
        let (iat, exp) = self.timestamps()?;

        let claims = AccessTokenClaims {
            sub: format!("client:{client_id}"),
            iss: self.issuer.clone(),
            aud: self.config.audience.clone(),
            iat,
            exp,
            roles: Vec::new(),
            email: None,
            scope: scope.to_string(),
            token_use: "access".into(),
            principal_type: "client".into(),
            client_id: Some(client_id.to_string()),
            extra: Default::default(),
        };

        self.sign(&claims)
    }

    fn timestamps(&self) -> Result<(u64, u64), OidcError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| OidcError::Internal(format!("system clock error: {e}")))?
            .as_secs();

        let exp = now
            .checked_add(self.config.token_ttl_secs)
            .ok_or_else(|| OidcError::Configuration("token TTL overflows exp claim".into()))?;
        Ok((now, exp))
    }

    fn sign(&self, claims: &AccessTokenClaims) -> Result<String, OidcError> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.key_pair.kid().to_string());

        encode(&header, &claims, self.key_pair.encoding_key())
            .map_err(|e| OidcError::Internal(format!("failed to sign JWT: {e}")))
    }

    pub fn token_ttl_secs(&self) -> u64 {
        self.config.token_ttl_secs
    }
}

/// Claims issued by this embedded access-token issuer.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AccessTokenClaims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    pub token_use: String,
    pub principal_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl r2e_security::jwt::JwtClaimSet for AccessTokenClaims {
    fn subject(&self) -> Option<&str> {
        Some(&self.sub)
    }
}

/// Scopes granted by default to the development password grant, unless
/// narrowed with `OidcServer::password_grant_scopes`.
pub const DEFAULT_USER_SCOPE: &str = "openid profile email roles";

/// RFC 6749 §3.3 `scope-token`: `%x21 / %x23-5B / %x5D-7E`, at least one char.
pub(crate) fn valid_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// Resolve the granted scope against a client's allowlist.
///
/// A request that omits `scope` receives the whole allowlist (empty when the
/// client declares none — fail closed). A request that names a scope outside
/// the allowlist is rejected with `invalid_scope` (RFC 6749 §4.1.2.1 / §5.2).
pub(crate) fn resolve_scope(
    requested: Option<&str>,
    allowed: &BTreeSet<String>,
) -> Result<String, OidcError> {
    let Some(requested) = requested else {
        return Ok(allowed.iter().cloned().collect::<Vec<_>>().join(" "));
    };

    let mut scopes = requested.split_whitespace().collect::<Vec<_>>();
    scopes.sort_unstable();
    scopes.dedup();

    if let Some(rejected) = scopes.iter().find(|scope| !allowed.contains(**scope)) {
        return Err(OidcError::InvalidScope(format!(
            "scope `{rejected}` is not allowed for this client"
        )));
    }
    Ok(scopes.join(" "))
}

pub(crate) fn has_scope(scope: &str, required: &str) -> bool {
    scope
        .split_whitespace()
        .any(|candidate| candidate == required)
}

pub(crate) fn filter_extra_claims(
    extra_claims: &std::collections::HashMap<String, serde_json::Value>,
) -> std::collections::HashMap<String, serde_json::Value> {
    const RESERVED: &[&str] = &[
        "sub",
        "iss",
        "aud",
        "iat",
        "exp",
        "nbf",
        "jti",
        "roles",
        "email",
        "scope",
        "token_use",
        "principal_type",
        "client_id",
    ];

    extra_claims
        .iter()
        .filter_map(|(k, v)| {
            if RESERVED.contains(&k.as_str()) {
                tracing::warn!(claim = %k, "Ignoring reserved claim in extra_claims");
                None
            } else {
                Some((k.clone(), v.clone()))
            }
        })
        .collect()
}
